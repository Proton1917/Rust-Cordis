//! 核心侧唯一内核的远端组件、资源路由与连接生命周期。

mod generation;
mod handlers;
mod lifecycle;
mod metadata;
mod model;
mod routing;

pub use model::{
    Failure, GroupSnapshot, GroupSpec, InstanceSnapshot, InstanceSpec, LogOrigin, LogRecord,
    LogSink, ManagerOptions, ReplaceReport, RestartPolicy, Snapshot,
};
pub use routing::{Arguments, CallContext, LocalService};

use crate::Peer;
use cordis::{ComponentDef, Context, Fiber, Inject};
use cordis_up::{
    Value, WireError, payload,
    rpc::{Connection, Handler, HandlerFuture, Outgoing, Payload, RemoteObject},
};
use generation::{
    ContextEntry, Generation, InstanceData, RemoteComponent, RemoteEffect, ScopeRecord, ViewLease,
};
use metadata::{MetadataLease, saved_error};
use model::Interface;
use parking_lot::Mutex;
use routing::Route;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

pub type Result<T = ()> = std::result::Result<T, WireError>;

pub(super) fn error(code: u32, message: impl Into<String>) -> WireError {
    WireError {
        code,
        message: message.into(),
        detail: Value::Null,
    }
}
pub(super) fn kernel_error(problem: cordis::Error) -> WireError {
    error(1, problem.to_string())
}
pub(super) fn kernel(problem: WireError) -> cordis::Error {
    cordis::Error::new(saved_error(&problem).to_string())
}
pub(super) fn value<T: Serialize + ?Sized>(data: &T) -> Result<Value> {
    Value::serialized(data).map_err(|problem| error(2, problem.to_string()))
}
pub(super) fn decode<T: DeserializeOwned>(data: &Value) -> Result<T> {
    data.deserialized()
        .map_err(|problem| error(2, problem.to_string()))
}

pub(super) struct GroupGeneration {
    connection: Connection,
    process: Option<Peer>,
    descriptor: payload::Describe,
    components: HashMap<(String, String), Arc<RemoteComponent>>,
    definitions: HashMap<(String, String), ComponentDef>,
    _metadata: Arc<MetadataLease>,
}

pub(super) struct Health {
    state: String,
    error: Option<WireError>,
}
pub(super) struct Group {
    name: String,
    specification: Mutex<Option<GroupSpec>>,
    current: Mutex<Arc<GroupGeneration>>,
    definitions: Mutex<HashMap<(String, String), ComponentDef>>,
    restarts: AtomicU32,
    health: Mutex<Health>,
}

pub(super) struct Instance {
    data: Arc<InstanceData>,
    fiber: Fiber,
}

#[derive(Default)]
pub(super) struct Catalog {
    groups: HashMap<String, Arc<Group>>,
    instances: HashMap<String, Arc<Instance>>,
    checkpoints: HashMap<String, payload::State>,
    reports: BTreeMap<String, ReplaceReport>,
}

pub(super) struct Routes {
    entries: HashMap<(u64, u64), Arc<Route>>,
    counters: HashMap<u64, ExportCounter>,
}

pub(super) struct ExportCounter {
    next: u64,
    active: usize,
}
pub(super) struct Effects {
    records: HashMap<(u64, u64), Weak<RemoteEffect>>,
    next: HashMap<u64, u64>,
}

trait Operation: Send {
    fn run(self: Box<Self>, manager: Manager) -> cordis::Task;
    fn cancel(self: Box<Self>);
}
struct Action<T, F> {
    callback: F,
    reply: oneshot::Sender<Result<T>>,
}
impl<T, F, Fut> Operation for Action<T, F>
where
    T: Send + 'static,
    F: FnOnce(Manager) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T>> + Send + 'static,
{
    fn run(self: Box<Self>, manager: Manager) -> cordis::Task {
        let Self { callback, reply } = *self;
        Box::pin(async move {
            let result = callback(manager.clone()).await;
            if let Err(Err(problem)) = reply.send(result) {
                manager
                    .0
                    .context
                    .logger()
                    .error(saved_error(&problem).to_string());
            }
        })
    }
    fn cancel(self: Box<Self>) {
        let Self { reply, .. } = *self;
        let _receiver = reply.send(Err(error(7, "管理器退出，排队操作已取消")));
    }
}
type Command = Box<dyn Operation>;
struct CommandQueue {
    accepting: bool,
    sender: mpsc::Sender<Command>,
}

pub(super) struct Core {
    context: Context,
    fiber: Fiber,
    options: ManagerOptions,
    next: AtomicU64,
    catalog: Mutex<Catalog>,
    contexts: Mutex<HashMap<u64, ContextEntry>>,
    scoped_views: Mutex<HashMap<u64, HashMap<usize, Arc<Route>>>>,
    scopes: Mutex<HashMap<u64, Weak<ScopeRecord>>>,
    effects: Mutex<Effects>,
    routes: Mutex<Routes>,
    metadata: Mutex<HashMap<u64, Arc<AtomicUsize>>>,
    stopping: AtomicBool,
    commands: Mutex<CommandQueue>,
    stop: watch::Sender<bool>,
    finished: watch::Sender<Option<Result<()>>>,
    log_sink: Mutex<Option<LogSink>>,
}

#[derive(Clone)]
pub struct Manager(Arc<Core>);

impl Manager {
    pub async fn new(parent: Context, options: ManagerOptions) -> Result<Self> {
        if options.operation_queue_capacity == 0 {
            return Err(error(2, "管理操作队列容量需要为正整数"));
        }
        let (sender, receiver) = mpsc::channel(options.operation_queue_capacity);
        let available = Arc::new(Mutex::new(None));
        let ready = available.clone();
        let definition =
            ComponentDef::new("cordis-supervisor", Inject::new(), move |context, ()| {
                *ready.lock() = Some(context);
                async { Ok(()) }
            });
        let fiber = parent.mount_with(&definition, ()).map_err(kernel_error)?;
        fiber.settled().await.map_err(kernel_error)?;
        let context = available
            .lock()
            .take()
            .expect("supervisor context initialized");
        let (stop, _) = watch::channel(false);
        let (finished, _) = watch::channel(None);
        let manager = Self(Arc::new(Core {
            context: context.clone(),
            fiber,
            options,
            next: AtomicU64::new(1),
            catalog: Mutex::new(Catalog::default()),
            contexts: Mutex::new(HashMap::new()),
            scoped_views: Mutex::new(HashMap::new()),
            scopes: Mutex::new(HashMap::new()),
            effects: Mutex::new(Effects {
                records: HashMap::new(),
                next: HashMap::new(),
            }),
            routes: Mutex::new(Routes {
                entries: HashMap::new(),
                counters: HashMap::new(),
            }),
            metadata: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
            commands: Mutex::new(CommandQueue {
                accepting: true,
                sender,
            }),
            stop,
            finished,
            log_sink: Mutex::new(None),
        }));
        let retained = manager.clone();
        context
            .effect("supervisor-shutdown", move |scope| {
                scope.defer_async(move || async move {
                    retained.shutdown_contents().await.map_err(kernel)
                })
            })
            .map_err(kernel_error)?;
        let worker = Arc::downgrade(&manager.0);
        let mut shutdown = manager.0.stop.subscribe();
        let receiver = Mutex::new(Some(receiver));
        context
            .effect_async("supervisor-operations", move |_| {
                let mut receiver = receiver.lock().take().expect("single operation worker");
                async move {
                    loop {
                        let Some(core) = worker.upgrade() else {
                            return Ok(());
                        };
                        let manager = Manager(core);
                        if *shutdown.borrow() {
                            while let Ok(command) = receiver.try_recv() {
                                command.cancel();
                            }
                            let result = manager.close_all().await;
                            manager.0.finished.send_replace(Some(result));
                            return Ok(());
                        }
                        tokio::select! {
                            biased;
                            _ = shutdown.changed() => {},
                            command = receiver.recv() => match command {
                                Some(command) if *shutdown.borrow() => command.cancel(),
                                Some(command) => command.run(manager).await,
                                None => return Ok(()),
                            }
                        }
                    }
                }
            })
            .map_err(kernel_error)?;
        Ok(manager)
    }

    pub fn context(&self) -> Context {
        self.0.context.clone()
    }
    pub fn set_log_sink(&self, sink: LogSink) {
        *self.0.log_sink.lock() = Some(sink);
    }
    pub(super) fn check_view(&self, view: &Context) -> Result<()> {
        if view.fiber() == self.0.fiber {
            Ok(())
        } else {
            Err(error(2, "挂载视图需要从Manager.context派生"))
        }
    }
    pub(super) fn running(&self) -> Result<()> {
        if self.0.stopping.load(Ordering::Acquire) {
            Err(error(4, "核心管理器正在退出"))
        } else {
            Ok(())
        }
    }
    async fn transaction<T, F, Fut>(&self, action: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Manager) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
    {
        let (reply, result) = oneshot::channel();
        let work: Command = Box::new(Action {
            callback: action,
            reply,
        });
        let submitted = {
            let queue = self.0.commands.lock();
            if !queue.accepting {
                return Err(error(4, "核心管理器正在退出"));
            }
            queue.sender.try_send(work)
        };
        submitted.map_err(|problem| match problem {
            mpsc::error::TrySendError::Full(_) => error(8, "核心管理操作队列已满"),
            mpsc::error::TrySendError::Closed(_) => error(4, "核心管理操作任务已结束"),
        })?;
        result
            .await
            .map_err(|_| error(4, "核心管理操作任务已结束"))?
    }
    pub(super) fn id(&self) -> u64 {
        self.0.next.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn group(&self, name: &str) -> Result<Arc<Group>> {
        self.0
            .catalog
            .lock()
            .groups
            .get(name)
            .cloned()
            .ok_or_else(|| error(4, "连接分组缺席"))
    }

    pub(super) fn instance(&self, name: &str) -> Result<Arc<Instance>> {
        self.0
            .catalog
            .lock()
            .instances
            .get(name)
            .cloned()
            .ok_or_else(|| error(4, "实例名称缺席"))
    }

    fn generation(&self, connection: u64, context: u64) -> Result<ViewLease> {
        let contexts = self.0.contexts.lock();
        let entry = contexts
            .get(&context)
            .ok_or_else(|| error(4, "上下文句柄已退役"))?;
        let generation = entry
            .generation
            .upgrade()
            .ok_or_else(|| error(4, "上下文纪元已退役"))?;
        if generation.component.connection.id() != connection
            || !generation.component.connection.is_alive()
        {
            return Err(error(4, "上下文连接已结束或属于另一条连接"));
        }
        Ok(ViewLease {
            id: context,
            context: entry.view.clone(),
            generation,
        })
    }

    pub(super) async fn lifecycle_request(
        &self,
        connection: &Connection,
        method: &str,
        params: Value,
        duration: Duration,
    ) -> Result<Payload> {
        let result = connection.request(method, params, duration).await;
        if let Err(problem) = &result
            && problem.code == 6
        {
            connection.terminate(&format!("{method} 生命周期请求超时"));
        }
        result
    }

    pub fn snapshot(&self) -> Snapshot {
        let (groups, instances, reports) = {
            let catalog = self.0.catalog.lock();
            (
                catalog.groups.values().cloned().collect::<Vec<_>>(),
                catalog.instances.values().cloned().collect::<Vec<_>>(),
                catalog.reports.values().cloned().collect(),
            )
        };
        let mut groups: Vec<_> = groups
            .iter()
            .map(|group| {
                let current = group.current.lock().clone();
                let health = group.health.lock();
                GroupSnapshot {
                    name: group.name.clone(),
                    connection: Some(current.connection.id()),
                    pid: current.process.as_ref().map(Peer::pid),
                    state: if !current.connection.is_alive()
                        && health.state == "running"
                        && !self.0.stopping.load(Ordering::Acquire)
                    {
                        "failed".into()
                    } else {
                        health.state.clone()
                    },
                    restarts: group.restarts.load(Ordering::Acquire),
                    plugins: current.descriptor.plugins.clone(),
                    error: health.error.as_ref().map(Failure::from),
                }
            })
            .collect();
        groups.sort_by(|left, right| left.name.cmp(&right.name));
        let mut instances: Vec<_> = instances
            .iter()
            .map(|instance| {
                let (revision, bindings) = match instance.fiber.epoch() {
                    cordis::Epoch::Inactive => (None, Vec::new()),
                    cordis::Epoch::Active { revision, impls } => (Some(revision), impls),
                };
                InstanceSnapshot {
                    name: instance.data.name.clone(),
                    group: instance.data.group.clone(),
                    plugin: instance.data.plugin.clone(),
                    component: instance.data.component.clone(),
                    state: format!("{:?}", instance.fiber.state()).to_lowercase(),
                    context: instance
                        .data
                        .current
                        .lock()
                        .upgrade()
                        .map(|current| current.id),
                    revision,
                    bindings,
                    error: instance
                        .data
                        .error
                        .lock()
                        .as_ref()
                        .map(|problem| Failure::from(&problem.value)),
                }
            })
            .collect();
        instances.sort_by(|left, right| left.name.cmp(&right.name));
        Snapshot {
            groups,
            instances,
            last_replace: reports,
        }
    }

    pub async fn shutdown(&self) -> Result<()> {
        let result = self.shutdown_contents().await;
        let disposed = self.0.fiber.dispose().await.map_err(kernel_error);
        result.and(disposed)
    }
}
