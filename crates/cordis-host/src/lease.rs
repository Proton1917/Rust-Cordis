//! DOC-12 §4、ADR-0012：库租约覆盖插件对象，退役与归零共同决定句柄释放。

use cordis_abi::{Environment, Error, Result};
use cordis_process::cache::CacheCopy;
use futures_util::future::poll_fn;
use libloading::Library;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    task::{Poll, Waker},
};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UnloadPolicy {
    #[default]
    Never,
    WhenUnleased,
}

static NEXT: AtomicU64 = AtomicU64::new(1);
static RETIRED: AtomicU64 = AtomicU64::new(0);
static LIBRARIES: OnceLock<Mutex<HashMap<u64, Arc<Lease>>>> = OnceLock::new();
static READY: Mutex<Vec<Arc<Lease>>> = Mutex::new(Vec::new());

fn libraries() -> &'static Mutex<HashMap<u64, Arc<Lease>>> {
    LIBRARIES.get_or_init(Mutex::default)
}

struct Image {
    library: Option<Library>,
    copy: Option<CacheCopy>,
    waiters: Vec<Waker>,
    descriptor_result: Option<Result<()>>,
}

pub struct Lease {
    pub id: u64,
    retired: AtomicBool,
    count: AtomicUsize,
    policy: UnloadPolicy,
    image: Mutex<Image>,
}

impl Lease {
    pub(crate) fn register(library: Library, policy: UnloadPolicy, copy: CacheCopy) -> Arc<Self> {
        let lease = Arc::new(Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            retired: AtomicBool::new(false),
            count: AtomicUsize::new(1),
            policy,
            image: Mutex::new(Image {
                library: Some(library),
                copy: Some(copy),
                waiters: Vec::new(),
                descriptor_result: None,
            }),
        });
        libraries().lock().insert(lease.id, lease.clone());
        lease
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
    pub fn retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }
    pub fn closed(&self) -> bool {
        self.image.lock().library.is_none()
    }
    pub fn retire(&self) {
        if !self.retired.swap(true, Ordering::AcqRel) {
            RETIRED.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// DOC-12 §4、ADR-0012：异步等待所有拥有引用释放，归零时唤醒等待者。
    pub async fn quiet(&self) {
        poll_fn(|context| {
            let mut image = self.image.lock();
            if self.count() == 0 {
                return Poll::Ready(());
            }
            if !image
                .waiters
                .iter()
                .any(|waker| waker.will_wake(context.waker()))
            {
                image.waiters.push(context.waker().clone());
            }
            Poll::Pending
        })
        .await;
    }

    /// DOC-12 §11、ADR-0053：静止后归还文件租约，映像继续遵循指定卸载策略。
    pub fn reap(&self) {
        if self.retired() && self.count() == 0 {
            let (library, copy) = {
                let mut image = self.image.lock();
                let library = (self.policy == UnloadPolicy::WhenUnleased
                    && matches!(image.descriptor_result, Some(Ok(()))))
                .then(|| image.library.take());
                (library, image.copy.take())
            };
            if library.is_some() {
                libraries().lock().remove(&self.id);
                drop(library);
            }
            drop(copy);
        }
    }

    pub(crate) fn descriptor_result(&self) -> Result<()> {
        self.image
            .lock()
            .descriptor_result
            .clone()
            .unwrap_or_else(|| Err(Error::new(4, "插件描述符尚未释放")))
    }

    /// SW-1：描述符握手完成后发布唯一副本，保持同一文件租约。
    pub(crate) fn publish_copy(&self, name: &str) -> Result<std::path::PathBuf> {
        self.image
            .lock()
            .copy
            .as_mut()
            .ok_or_else(|| Error::new(4, "库副本已退役"))?
            .publish(name)
            .map_err(|error| Error::new(1, error.to_string()))
    }
}

/// DOC-12 §4：活动库增加拥有租约，已经静止的退役库返回状态错误。
pub fn acquire(id: u64) -> Result<()> {
    if id == 0 {
        return Ok(());
    }
    let lease = libraries()
        .lock()
        .get(&id)
        .cloned()
        .ok_or_else(|| Error::new(4, "插件库句柄已退役"))?;
    lease
        .count
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            if count == 0 && lease.retired() {
                None
            } else {
                count.checked_add(1)
            }
        })
        .map(|_| ())
        .map_err(|_| Error::new(4, "插件库已进入静止状态"))
}

/// DOC-12 §4：消费一个拥有租约，最终释放通知静止等待者。
pub fn release(id: u64) -> Result<()> {
    if id == 0 {
        return Ok(());
    }
    let lease = libraries()
        .lock()
        .get(&id)
        .cloned()
        .ok_or_else(|| Error::new(4, "插件库句柄已退役"))?;
    let previous = lease
        .count
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            count.checked_sub(1)
        })
        .map_err(|_| Error::new(2, "插件库租约已经归零"))?;
    if previous == 1 {
        if lease.retired() {
            READY.lock().push(lease.clone());
        }
        let waiters = std::mem::take(&mut lease.image.lock().waiters);
        for waiter in waiters {
            waiter.wake();
        }
    }
    Ok(())
}

pub fn retired_images() -> u64 {
    RETIRED.load(Ordering::Relaxed)
}

pub fn reap_ready() {
    let values = std::mem::take(&mut *READY.lock());
    for lease in values {
        lease.reap();
    }
}

pub(crate) struct Descriptor {
    pub raw: *const cordis_abi::PluginV1,
    pub lease: Arc<Lease>,
    pub environment: Environment,
}

// SAFETY: DOC-12 §2、§10，描述符只读，组件函数满足Send+Sync，租约覆盖指针寿命。
unsafe impl Send for Descriptor {}
// SAFETY: DOC-12 §2、§10，共享读取由库租约保持有效。
unsafe impl Sync for Descriptor {}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: DOC-12 §10，在描述符释放前复制源侧release函数指针。
        let release_descriptor = unsafe { (*self.raw).release };
        // SAFETY: DOC-12 §3、§10，唯一描述符所有权在库租约归还前交给源侧。
        let finish = || unsafe {
            self.environment
                .call_unit(|err| release_descriptor(self.raw, err))
        };
        // 同步收束插件工作时交还宿主调度线程，同组其他实例和清理回调仍可执行。
        let result = if self.lease.policy == UnloadPolicy::WhenUnleased
            && tokio::runtime::Handle::try_current().is_ok_and(|handle| {
                handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
            }) {
            tokio::task::block_in_place(finish)
        } else {
            finish()
        };
        self.lease.image.lock().descriptor_result = Some(result.clone());
        let released = release(self.lease.id);
        if let Err(error) = result.and(released) {
            self.environment.report(error);
        }
    }
}
