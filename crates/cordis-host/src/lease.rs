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
static CLOSED: AtomicU64 = AtomicU64::new(0);
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
    #[cfg(target_os = "linux")]
    identity: Option<MappingIdentity>,
}

impl Lease {
    pub(crate) fn register(library: Library, policy: UnloadPolicy, copy: CacheCopy) -> Arc<Self> {
        #[cfg(target_os = "linux")]
        let identity = MappingIdentity::read(copy.path());
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
            #[cfg(target_os = "linux")]
            identity,
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
            #[cfg(target_os = "linux")]
            record_mapping(self.id, self.identity);
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
                let library = if self.policy == UnloadPolicy::WhenUnleased
                    && matches!(image.descriptor_result, Some(Ok(())))
                {
                    image.library.take()
                } else {
                    None
                };
                (library, image.copy.take())
            };
            if let Some(library) = library {
                libraries().lock().remove(&self.id);
                drop(library);
                #[cfg(target_os = "linux")]
                if let Some(mapping) = retired_mappings().lock().get_mut(&self.id) {
                    mapping.closed = true;
                }
                let _closed_total = CLOSED.fetch_add(1, Ordering::Relaxed) + 1;
                #[cfg(target_os = "linux")]
                periodic_mapping_cleanup(_closed_total, || {
                    let retained: Vec<_> = libraries().lock().values().cloned().collect();
                    let _ = mapped_retired_images(&retained);
                });
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

pub fn retirement_status(measure_mappings: bool) -> cordis_up::payload::LibraryRetirement {
    let retained: Vec<_> = libraries().lock().values().cloned().collect();
    let mut status = cordis_up::payload::LibraryRetirement {
        retired_total: retired_images(),
        closed_total: CLOSED.load(Ordering::Relaxed),
        ..Default::default()
    };
    for lease in &retained {
        if lease.retired() {
            let image = lease.image.lock();
            status.open_retired_handles += u64::from(image.library.is_some());
            status.retired_cache_pins += u64::from(image.copy.is_some());
            status.retired_leases += lease.count() as u64;
        }
    }
    if measure_mappings {
        status.mapped_retired_images = mapped_retired_images(&retained);
    }
    status
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MappingIdentity {
    major: u64,
    minor: u64,
    inode: u64,
}

#[cfg(target_os = "linux")]
impl MappingIdentity {
    fn read(path: &std::path::Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).ok()?;
        let device = metadata.dev();
        Some(Self {
            major: ((device >> 8) & 0xfff) | ((device >> 32) & 0xfffff000),
            minor: (device & 0xff) | ((device >> 12) & 0xffffff00),
            inode: metadata.ino(),
        })
    }
}

#[cfg(target_os = "linux")]
struct RetiredMapping {
    identity: MappingIdentity,
    closed: bool,
}
#[cfg(target_os = "linux")]
static RETIRED_MAPPINGS: OnceLock<Mutex<HashMap<u64, RetiredMapping>>> = OnceLock::new();
#[cfg(target_os = "linux")]
static COMPLETE_MAPPINGS: AtomicBool = AtomicBool::new(true);

#[cfg(target_os = "linux")]
fn retired_mappings() -> &'static Mutex<HashMap<u64, RetiredMapping>> {
    RETIRED_MAPPINGS.get_or_init(Mutex::default)
}

#[cfg(target_os = "linux")]
fn record_mapping(id: u64, identity: Option<MappingIdentity>) {
    match identity {
        Some(identity) => {
            retired_mappings().lock().insert(
                id,
                RetiredMapping {
                    identity,
                    closed: false,
                },
            );
        }
        None => {
            COMPLETE_MAPPINGS.store(false, Ordering::Release);
        }
    }
}

#[cfg(target_os = "linux")]
fn periodic_mapping_cleanup(closed_total: u64, cleanup: impl FnOnce()) {
    // Direct host users can retire indefinitely without ever requesting status.
    // Bound unobserved closed history with one sweep per 256 returned handles.
    if closed_total.is_multiple_of(256) {
        cleanup();
    }
}

#[cfg(target_os = "linux")]
fn mapped_files() -> Option<std::collections::HashSet<MappingIdentity>> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    let mut identities = std::collections::HashSet::new();
    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let (major, minor) = fields.nth(3)?.split_once(':')?;
        let inode = fields.next()?.parse::<u64>().ok()?;
        if inode != 0 {
            identities.insert(MappingIdentity {
                major: u64::from_str_radix(major, 16).ok()?,
                minor: u64::from_str_radix(minor, 16).ok()?,
                inode,
            });
        }
    }
    Some(identities)
}

#[cfg(target_os = "linux")]
fn mapped_retired_images(retained: &[Arc<Lease>]) -> Option<u64> {
    let complete = COMPLETE_MAPPINGS.load(Ordering::Acquire);
    let mapped = mapped_files()?;
    let active = retained
        .iter()
        .filter(|lease| !lease.retired())
        .filter_map(|lease| lease.identity)
        .collect();
    let mut retired = retired_mappings().lock();
    observed_retired_images(&mut retired, &mapped, &active, complete)
}

#[cfg(target_os = "linux")]
fn observed_retired_images(
    retired: &mut HashMap<u64, RetiredMapping>,
    mapped: &std::collections::HashSet<MappingIdentity>,
    active: &std::collections::HashSet<MappingIdentity>,
    complete: bool,
) -> Option<u64> {
    // Every load owns a newly allocated cache-copy inode. A closed historical
    // identity now owned by an active copy has been reused after unmapping.
    retired.retain(|_, entry| {
        !entry.closed || (mapped.contains(&entry.identity) && !active.contains(&entry.identity))
    });
    complete.then(|| {
        retired
            .values()
            .filter(|entry| mapped.contains(&entry.identity))
            .map(|entry| entry.identity)
            .collect::<std::collections::HashSet<_>>()
            .len() as u64
    })
}

#[cfg(not(target_os = "linux"))]
fn mapped_retired_images(_: &[Arc<Lease>]) -> Option<u64> {
    None
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

#[cfg(all(test, target_os = "linux"))]
mod mapping_tests {
    use super::*;

    #[test]
    fn linux_mapping_observation_matches_real_device_and_inode() {
        let executable = std::env::current_exe().unwrap();
        let identity = MappingIdentity::read(&executable).unwrap();
        assert!(mapped_files().unwrap().contains(&identity));
    }

    #[test]
    fn mapping_observation_reconciles_closed_history_and_reused_inodes() {
        let identity = MappingIdentity {
            major: 1,
            minor: 2,
            inode: 3,
        };
        let mapped = std::collections::HashSet::from([identity]);
        let mut retired = HashMap::from([
            (
                1,
                RetiredMapping {
                    identity,
                    closed: true,
                },
            ),
            (
                2,
                RetiredMapping {
                    identity,
                    closed: true,
                },
            ),
        ]);
        assert_eq!(
            observed_retired_images(&mut retired, &mapped, &Default::default(), true),
            Some(1)
        );
        assert_eq!(
            observed_retired_images(&mut retired, &mapped, &mapped, true),
            Some(0)
        );
        assert!(retired.is_empty());
        retired.insert(
            3,
            RetiredMapping {
                identity,
                closed: false,
            },
        );
        assert_eq!(
            observed_retired_images(&mut retired, &mapped, &Default::default(), true),
            Some(1)
        );
        retired.get_mut(&3).unwrap().closed = true;
        assert_eq!(
            observed_retired_images(&mut retired, &Default::default(), &Default::default(), true),
            Some(0)
        );
        assert!(retired.is_empty());
    }

    #[test]
    fn closed_mapping_history_is_reclaimed_without_status_polling() {
        let executable = MappingIdentity::read(&std::env::current_exe().unwrap()).unwrap();
        let mut retired = HashMap::from([(
            0,
            RetiredMapping {
                identity: executable,
                closed: true,
            },
        )]);
        let mut observations = 0;
        let mut peak = 0;
        for closed in 1..=768 {
            retired.insert(
                closed,
                RetiredMapping {
                    // A device identity outside Linux's device-number representation
                    // supplies a closed, unmapped historical image for this replay.
                    identity: MappingIdentity {
                        major: u64::MAX,
                        minor: 0,
                        inode: closed,
                    },
                    closed: true,
                },
            );
            peak = peak.max(retired.len());
            periodic_mapping_cleanup(closed, || {
                observations += 1;
                let mapped = mapped_files().unwrap();
                observed_retired_images(&mut retired, &mapped, &Default::default(), true);
            });
        }
        assert_eq!(
            observations, 3,
            "cleanup work is amortized across close transitions"
        );
        assert_eq!(
            peak, 257,
            "one mapped image plus at most 256 unobserved closes"
        );
        assert_eq!(
            retired.len(),
            1,
            "unmapped history is released without status requests"
        );
        assert_eq!(
            retired[&0].identity, executable,
            "actual mapped code stays accounted for"
        );
    }

    #[test]
    fn incomplete_mapping_measurement_still_reclaims_known_history() {
        let identity = MappingIdentity {
            major: 1,
            minor: 2,
            inode: 3,
        };
        let mut retired = HashMap::from([
            (
                1,
                RetiredMapping {
                    identity,
                    closed: true,
                },
            ),
            (
                2,
                RetiredMapping {
                    identity,
                    closed: false,
                },
            ),
        ]);
        assert_eq!(
            observed_retired_images(
                &mut retired,
                &Default::default(),
                &Default::default(),
                false
            ),
            None,
        );
        assert_eq!(retired.len(), 1);
        assert!(!retired[&2].closed);
    }
}
