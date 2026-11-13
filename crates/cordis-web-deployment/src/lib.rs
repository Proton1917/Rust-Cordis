//! DOC-15 §11、ADR-0041：HTTP资源登记、页面连接和真实内核父子实例的装配。
#![forbid(unsafe_code)]

mod pages;
mod registry;

use cordis::{ComponentDef, Context, Fiber, Inject, NamedProvision};
use cordis_channels::Channels;
use cordis_supervisor::manager::{Arguments, CallContext, LocalReply, LocalService, Manager};
use cordis_up::{
    Value, WireError, payload,
    rpc::{Connection, Handler, HandlerFuture, Outgoing, Payload},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;

pub type Result<T> = std::result::Result<T, WireError>;

#[derive(Clone)]
/// DOC-15 §11.1：从已验证插件清单取得资源，parent指定全栈后端实例。
pub struct Spec {
    pub view: Context,
    pub instance: String,
    pub name: String,
    pub version: String,
    pub assets: String,
    pub mount: String,
    pub entry: String,
    pub component: String,
    pub parent: Option<String>,
    pub config: Value,
}

struct Owner {
    spec: Arc<Mutex<Spec>>,
    fiber: Fiber,
    revision: Arc<AtomicU64>,
    ready: watch::Sender<Option<Result<u64>>>,
}
struct Live {
    spec: Spec,
    context: Context,
    registration: registry::Registration,
    children: Arc<Mutex<BTreeMap<u64, Fiber>>>,
    mounting: Mutex<HashMap<u64, watch::Sender<Option<Result<()>>>>>,
}
#[derive(Clone)]
struct Page {
    source: Value,
    group: String,
    connection: Connection,
    plugins: Arc<Mutex<HashMap<String, (String, String)>>>,
    provides: Arc<Mutex<Vec<String>>>,
}

/// DOC-15 §11.4：冻结句柄与原配置由上层完整替换事务共同持有。
pub struct Change {
    deployment: Weak<Deployment>,
    pub previous: Spec,
    revision: u64,
    snapshot: cordis_supervisor::manager::FrozenSnapshot,
    restored: AtomicU64,
}

impl Drop for Change {
    fn drop(&mut self) {
        if let Some(deployment) = self.deployment.upgrade() {
            let mut saved = deployment.saved.lock();
            for name in self.snapshot.states().keys() {
                saved.remove(name);
            }
        }
    }
}

pub struct Deployment {
    weak: Weak<Self>,
    manager: Arc<Manager>,
    channels: Channels,
    allowed: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    origins: Mutex<HashMap<(u64, u64), String>>,
    owners: Mutex<BTreeMap<String, Owner>>,
    live: Mutex<BTreeMap<String, Arc<Live>>>,
    pages: Mutex<HashMap<u64, Page>>,
    saved: Mutex<HashMap<String, payload::State>>,
    next: AtomicU64,
}

impl Deployment {
    /// ADR-0041：弱扩展入口接入既有监管连接，运行时持有完整部署生命周期。
    pub fn new(
        manager: Arc<Manager>,
        allowed: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> Result<Arc<Self>> {
        let channels = Channels::new(Default::default())?;
        let deployment = Arc::new_cyclic(|weak| Self {
            weak: weak.clone(),
            manager: manager.clone(),
            channels,
            allowed,
            origins: Mutex::new(HashMap::new()),
            owners: Mutex::new(BTreeMap::new()),
            live: Mutex::new(BTreeMap::new()),
            pages: Mutex::new(HashMap::new()),
            saved: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        });
        let extension: Arc<dyn Handler> = deployment.clone();
        manager.set_handler_extension(Arc::downgrade(&extension));
        Ok(deployment)
    }

    /// DOC-16 §11：运行时以配置中的forwarder身份授权物理连接，并提供该视图的服务能力。
    pub fn forwarder(&self, view: &Context) -> Result<NamedProvision> {
        self.manager.provide_builtin_in(
            view,
            "cordis.forward@1.0",
            vec!["open".into(), "frame".into(), "close".into()],
            Arc::new(Permission),
        )
    }

    /// DOC-15 §11.3：资源登记Owner由内核依赖驱动；全栈Owner同时依赖指定后端的presence。
    pub fn mount(self: &Arc<Self>, spec: Spec) -> Result<Fiber> {
        let name = spec.instance.clone();
        let parent_view = spec.view.clone();
        let mut inject = Inject::new().with_named("cordis.web@1");
        if let Some(parent) = &spec.parent {
            inject = inject.with_named(format!("cordis.instance.{parent}"));
        }
        let current = Arc::new(Mutex::new(spec));
        let revision = Arc::new(AtomicU64::new(1));
        let ready = watch::channel(None).0;
        let (configuration, generation, signal) =
            (current.clone(), revision.clone(), ready.clone());
        let weak = Arc::downgrade(self);
        let definition = ComponentDef::new(
            format!("web-owner-{name}"),
            inject,
            move |context, _revision: u64| {
                let (weak, current, revision, ready) = (
                    weak.clone(),
                    configuration.clone(),
                    generation.clone(),
                    signal.clone(),
                );
                async move {
                    let Some(deployment) = weak.upgrade() else {
                        return Err(cordis::Error::new("网页部署已退出"));
                    };
                    let spec = current.lock().clone();
                    let expected = revision.load(Ordering::Acquire);
                    let result = deployment.activate(context, spec).await;
                    ready.send_replace(Some(
                        result.as_ref().map(|_| expected).map_err(Clone::clone),
                    ));
                    result.map_err(kernel)
                }
            },
        );
        let fiber = parent_view.mount_with(&definition, 1u64).map_err(wire)?;
        self.owners.lock().insert(
            name,
            Owner {
                spec: current,
                fiber: fiber.clone(),
                revision,
                ready,
            },
        );
        Ok(fiber)
    }

    /// DOC-15 §11.4：先冻结页面状态，配置替换交由上层与后端提交协同。
    pub async fn prepare_change(&self, name: &str, candidate: Spec) -> Result<Change> {
        let names = self.child_names(name);
        let snapshot = self.manager.freeze_instances(&names).await?;
        let owners = self.owners.lock();
        let owner = owners
            .get(name)
            .ok_or_else(|| WireError::new(4, "网页实例缺席"))?;
        let previous = std::mem::replace(&mut *owner.spec.lock(), candidate);
        let revision = owner.revision.fetch_add(1, Ordering::AcqRel) + 1;
        owner.ready.send_replace(None);
        self.saved.lock().extend(snapshot.states().clone());
        Ok(Change {
            deployment: self.weak.clone(),
            previous,
            revision,
            snapshot,
            restored: AtomicU64::new(0),
        })
    }

    /// DOC-15 §11.4：撤回前端时保留页面状态与原配置，实际释放由remove完成。
    pub async fn prepare_removal(&self, name: &str) -> Result<Change> {
        let previous = self
            .specification(name)
            .ok_or_else(|| WireError::new(4, "网页实例缺席"))?;
        let snapshot = self
            .manager
            .freeze_instances(&self.child_names(name))
            .await?;
        self.saved.lock().extend(snapshot.states().clone());
        Ok(Change {
            deployment: self.weak.clone(),
            previous,
            revision: 0,
            snapshot,
            restored: AtomicU64::new(0),
        })
    }

    /// DOC-15 §11.4：纯网页版本直接更新Owner，全栈版本由后端presence变更触发重建。
    pub async fn finish_change(&self, name: &str, change: &Change) -> Result<()> {
        let (fiber, pure) = {
            let owners = self.owners.lock();
            let owner = owners
                .get(name)
                .ok_or_else(|| WireError::new(4, "网页实例缺席"))?;
            (owner.fiber.clone(), owner.spec.lock().parent.is_none())
        };
        if pure {
            fiber.update(change.revision).await.map_err(wire)?;
        }
        self.ready(name, change.revision).await
    }

    /// DOC-11回滚：恢复配置后重建原网页版本；可继续存在的原页面解除冻结。
    pub async fn rollback_change(&self, name: &str, change: &Change) -> Result<()> {
        if !self.owners.lock().contains_key(name) {
            self.saved.lock().extend(change.snapshot.states().clone());
            self.weak
                .upgrade()
                .ok_or_else(|| WireError::new(4, "网页部署已结束"))?
                .mount(change.previous.clone())?;
            self.manager.thaw_snapshot(&change.snapshot).await?;
            return self.wait_mounted(name).await;
        }
        if change.revision == 0 {
            self.manager.thaw_snapshot(&change.snapshot).await?;
            self.instance_fiber(name)?.settled().await.map_err(wire)?;
            return self.wait_mounted(name).await;
        }
        let restored = change.restored.load(Ordering::Acquire);
        if restored != 0 {
            self.manager.thaw_snapshot(&change.snapshot).await?;
            return self.restore_owner(name, change, restored).await;
        }
        self.saved.lock().extend(change.snapshot.states().clone());
        let (fiber, revision) = {
            let owners = self.owners.lock();
            let owner = owners
                .get(name)
                .ok_or_else(|| WireError::new(4, "网页实例缺席"))?;
            *owner.spec.lock() = change.previous.clone();
            let revision = owner.revision.fetch_add(1, Ordering::AcqRel) + 1;
            owner.ready.send_replace(None);
            (owner.fiber.clone(), revision)
        };
        self.manager.thaw_snapshot(&change.snapshot).await?;
        fiber.update(revision).await.map_err(wire)?;
        self.ready(name, revision).await
    }

    /// DOC-11回滚：等待当前加载结束，成功的恢复版本直接确认，失败Owner按恢复版本重新加载。
    async fn restore_owner(&self, name: &str, change: &Change, revision: u64) -> Result<()> {
        let (fiber, ready) = {
            let owners = self.owners.lock();
            let owner = owners
                .get(name)
                .ok_or_else(|| WireError::new(4, "网页实例缺席"))?;
            (owner.fiber.clone(), owner.ready.subscribe())
        };
        let previous = fiber.settled().await;
        let confirmed =
            matches!(ready.borrow().as_ref(), Some(Ok(current)) if *current >= revision);
        if previous.is_ok() && fiber.state() == cordis::FiberState::Active && confirmed {
            return Ok(());
        }
        self.saved.lock().extend(change.snapshot.states().clone());
        if let Err(problem) = fiber.update(revision).await {
            let message = match previous {
                Ok(()) => problem.to_string(),
                Err(previous) => format!("先前页面加载：{previous}；恢复加载：{problem}"),
            };
            return Err(WireError::new(1, message));
        }
        self.ready(name, revision).await
    }

    /// DOC-15 §11.4：后端回退前恢复待装配配置，Owner重建随后读取原版本。
    pub fn restore_configuration(&self, name: &str, change: &Change) -> Result<()> {
        self.saved.lock().extend(change.snapshot.states().clone());
        let owners = self.owners.lock();
        let Some(owner) = owners.get(name) else {
            return Ok(());
        };
        *owner.spec.lock() = change.previous.clone();
        let revision = owner.revision.fetch_add(1, Ordering::AcqRel) + 1;
        owner.ready.send_replace(None);
        change.restored.store(revision, Ordering::Release);
        Ok(())
    }

    pub async fn wait_mounted(&self, name: &str) -> Result<()> {
        let revision = self
            .owners
            .lock()
            .get(name)
            .ok_or_else(|| WireError::new(4, "网页实例缺席"))?
            .revision
            .load(Ordering::Acquire);
        self.ready(name, revision).await
    }

    pub fn specification(&self, name: &str) -> Option<Spec> {
        self.owners
            .lock()
            .get(name)
            .map(|owner| owner.spec.lock().clone())
    }

    pub fn instance_fiber(&self, name: &str) -> Result<Fiber> {
        self.owners
            .lock()
            .get(name)
            .map(|owner| owner.fiber.clone())
            .ok_or_else(|| WireError::new(4, "网页实例缺席"))
    }

    /// DOC-15 §11：核心状态等待以Owner本次发布的结果结束。
    async fn ready(&self, name: &str, revision: u64) -> Result<()> {
        let mut ready = self
            .owners
            .lock()
            .get(name)
            .ok_or_else(|| WireError::new(4, "网页实例缺席"))?
            .ready
            .subscribe();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(result) = ready.borrow_and_update().clone()
                    && result? >= revision
                {
                    return Ok(());
                }
                ready
                    .changed()
                    .await
                    .map_err(|_| WireError::new(4, "网页状态通知已结束"))?;
            }
        })
        .await
        .map_err(|_| WireError::new(6, "网页版本确认超时"))?
    }

    pub async fn remove(&self, name: &str) -> Result<()> {
        let owner = self.owners.lock().remove(name);
        if let Some(owner) = owner {
            owner.fiber.dispose().await.map_err(wire)?;
        }
        Ok(())
    }

    /// DOC-15 §11：纯网页父实例的状态直接取自实际内核Owner。
    pub fn snapshot(&self) -> Vec<cordis_supervisor::manager::InstanceSnapshot> {
        self.owners
            .lock()
            .values()
            .filter_map(|owner| {
                let spec = owner.spec.lock();
                if spec.parent.is_some() {
                    return None;
                }
                let (revision, bindings) = match owner.fiber.epoch() {
                    cordis::Epoch::Active { revision, impls } => (Some(revision), impls),
                    cordis::Epoch::Inactive => (None, Vec::new()),
                };
                let error = owner
                    .ready
                    .borrow()
                    .as_ref()
                    .and_then(|result| result.as_ref().err())
                    .map(Into::into);
                Some(cordis_supervisor::manager::InstanceSnapshot {
                    page_of: None,
                    name: spec.instance.clone(),
                    plugin: spec.name.clone(),
                    component: spec.component.clone(),
                    state: format!("{:?}", owner.fiber.state()).to_lowercase(),
                    revision,
                    bindings,
                    error,
                    group: spec.name.clone(),
                    context: None,
                    reference: None,
                })
            })
            .collect()
    }
}

struct Permission;
impl LocalService for Permission {
    fn invoke<'a>(
        &'a self,
        _context: &'a CallContext,
        _method: &'a str,
        _args: &'a Arguments,
    ) -> HandlerFuture<'a, Result<LocalReply>> {
        Box::pin(async {
            Err(WireError::new(3, "转发能力由原生宿主适配为channel消息"))
        })
    }
}

fn value<T: Serialize>(value: &T) -> Result<Value> {
    Value::serialized(value).map_err(|problem| WireError::new(2, problem.to_string()))
}
fn wire(problem: impl std::fmt::Display) -> WireError {
    WireError::new(1, problem.to_string())
}
fn kernel(problem: WireError) -> cordis::Error {
    cordis::Error::new(problem.to_string())
}
