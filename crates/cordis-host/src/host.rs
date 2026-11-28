//! DOC-15 Q0、§6.4：执行核心下发的原生操作，维持上下文、对象和库租约。

use crate::{
    abi, lease,
    library::{LibraryOptions, LibraryStore, NativeComponent, NativeLibrary},
    native::{Cancellation, NativeExecutor, Owned, RuntimeOwner},
    objects::Objects,
    wire::{Bridge, from_wire, to_wire},
};
use cordis_abi::{
    AbiUnit, Error, Object, RemoteFuture, Result, State, decode_value, encode, from_value,
    make_future, to_value,
};
use cordis_up::{
    Value, WireError, payload,
    rpc::{Connection, Handler, HandlerFuture, Outgoing, Payload},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;

mod client;
mod protocol;

#[derive(Clone, Debug)]
pub struct HostOptions {
    pub libraries: Vec<PathBuf>,
    pub library: LibraryOptions,
    pub request_timeout: Duration,
    pub native_workers: usize,
    pub max_active_callbacks: usize,
    pub max_callback_bytes: usize,
}

pub(crate) struct NotificationTask {
    pub cancel: Option<Cancellation>,
    pub done: tokio::sync::oneshot::Receiver<()>,
}

struct ContextData {
    base: u64,
    retiring: AtomicBool,
    _component: Owned<Arc<NativeComponent>>,
    config: Vec<u8>,
    inherited: Option<State>,
}

struct ContextRecord {
    data: Arc<ContextData>,
    notifications: Mutex<HashMap<u64, NotificationTask>>,
    phase: watch::Sender<ClosePhase>,
}

enum ClosePhase {
    Open,
    Closing,
    Draining,
    Closed(Result<()>),
}

impl ContextRecord {
    fn new(data: Arc<ContextData>) -> Self {
        Self {
            data,
            notifications: Mutex::new(HashMap::new()),
            phase: watch::channel(ClosePhase::Open).0,
        }
    }

    fn schedule_close(&self) -> bool {
        self.phase.send_if_modified(|phase| {
            if matches!(phase, ClosePhase::Open) {
                *phase = ClosePhase::Closing;
                return true;
            }
            false
        })
    }
}

struct RetiredLibrary {
    info: crate::library::LibraryInfo,
    lease: Arc<lease::Lease>,
}

pub(crate) struct Inner {
    pub forward: Arc<crate::forward::Forwarders>,
    options: HostOptions,
    store: LibraryStore,
    libraries: Mutex<HashMap<u64, NativeLibrary>>,
    retired: Mutex<HashMap<u64, RetiredLibrary>>,
    active: Mutex<HashMap<String, u64>>,
    connection: OnceLock<Connection>,
    process: AtomicU64,
    group: OnceLock<String>,
    next_notification: AtomicU64,
    callback_admission: Arc<crate::callback::Admission>,
    initialized: AtomicBool,
    contexts: Mutex<HashMap<u64, Arc<ContextRecord>>>,
    pub objects: Arc<Objects>,
    pub executor: NativeExecutor,
    pub runtime: tokio::runtime::Handle,
    started: Instant,
    stopping: AtomicBool,
    stopped: AtomicBool,
    _native_runtime: RuntimeOwner,
}

#[derive(Clone)]
pub struct Host(pub(crate) Arc<Inner>);

impl Host {
    /// ADR-0051：调度线程交接属于原生宿主，借用回调在当前调用期间结束。
    pub fn block_in_place<T>(&self, body: impl FnOnce() -> T) -> T {
        self.0.executor.block_in_place(body)
    }

    /// DOC-15 CP-1：初始化宿主数据和独立原生执行器，运行时构造错误直接返回。
    pub fn new(options: HostOptions, io: tokio::runtime::Handle) -> Result<Self> {
        let callback_admission = crate::callback::Admission::new(
            options.max_active_callbacks,
            options.max_callback_bytes,
        )?;
        let store = LibraryStore::new(options.library.clone(), &options.libraries)?;
        let native = RuntimeOwner::new(options.native_workers)?;
        let executor = NativeExecutor::new(native.handle(), io.clone());
        Ok(Self(Arc::new(Inner {
            forward: Arc::new(crate::forward::Forwarders::default()),
            options,
            store,
            libraries: Mutex::new(HashMap::new()),
            retired: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            connection: OnceLock::new(),
            process: AtomicU64::new(0),
            group: OnceLock::new(),
            next_notification: AtomicU64::new(1),
            callback_admission,
            initialized: AtomicBool::new(false),
            contexts: Mutex::new(HashMap::new()),
            objects: Arc::new(Objects::new(executor.clone())),
            executor,
            runtime: io,
            started: Instant::now(),
            stopping: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            _native_runtime: native,
        })))
    }

    pub fn stopped(&self) -> bool {
        self.0.stopped.load(Ordering::Acquire)
    }

    pub fn objects(&self) -> &Arc<Objects> {
        &self.0.objects
    }

    /// DOC-12 §10：读取候选静态描述后释放描述符和库租约。
    pub async fn describe_file(&self, path: PathBuf) -> Result<payload::Plugin> {
        let host = self.clone();
        self.0
            .executor
            .run(
                async move {
                    let prepared = host.prepare(&path)?;
                    let mut description = prepared.plugin;
                    host.retire(prepared.library).await?;
                    description.library = None;
                    Ok(description)
                },
                false,
            )
            .await
    }

    pub(crate) fn connection(&self) -> Result<Connection> {
        self.0
            .connection
            .get()
            .cloned()
            .ok_or_else(|| Error::new(4, "宿主连接准备中"))
    }

    fn bridge(&self) -> Result<Bridge> {
        Ok(Bridge {
            objects: self.0.objects.clone(),
            process: self.0.process.load(Ordering::Acquire),
            executor: self.0.executor.clone(),
            timeout: self.0.options.request_timeout,
        })
    }

    fn context(&self, id: u64) -> Result<Arc<ContextRecord>> {
        self.0
            .contexts
            .lock()
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::new(4, "上下文已释放"))
    }

    fn component(
        &self,
        plugin: &str,
        name: &str,
        library: Option<u64>,
    ) -> Result<Arc<NativeComponent>> {
        let id = match library {
            Some(id) => id,
            None => *self
                .0
                .active
                .lock()
                .get(plugin)
                .ok_or_else(|| Error::new(4, "插件来源尚待核心指定"))?,
        };
        let libraries = self.0.libraries.lock();
        let library = libraries
            .get(&id)
            .ok_or_else(|| Error::new(4, "原生库编号已退役"))?;
        if library.name() != plugin {
            return Err(Error::new(2, "原生库编号与插件名不匹配"));
        }
        library.component(name)
    }

    /// DOC-11 SW-1、SW-2：准备核心指定的副本，将描述符登记为候选库。
    fn prepare(&self, path: &Path) -> Result<payload::LibraryPrepared> {
        let library = self
            .0
            .store
            .open(path, abi::environment())
            .map_err(|error| error.error)?;
        let info = library.info();
        let prepared = payload::LibraryPrepared {
            library: info.id,
            plugin: info.description(),
        };
        self.0.libraries.lock().insert(info.id, library);
        Ok(prepared)
    }

    /// DOC-15 Q0：在一致的目录快照中汇报活动与退役库，供核心决定后续处理。
    fn library_status(&self) -> payload::LibraryStatus {
        let records: Vec<_> = {
            let libraries = self.0.libraries.lock();
            let retired = self.0.retired.lock();
            libraries
                .values()
                .map(|library| (library.info(), library.lease.clone()))
                .chain(
                    retired
                        .values()
                        .map(|library| (library.info.clone(), library.lease.clone())),
                )
                .collect()
        };
        let process = self.0.process.load(Ordering::Acquire);
        let group = self.0.group.get().cloned().unwrap_or_default();
        let mut libraries = records
            .into_iter()
            .map(|(info, lease)| {
                let closed = lease.closed();
                let retired = lease.retired();
                let leases = lease.count();
                cordis_types::LibraryInfo {
                    id: info.id,
                    name: info.name,
                    version: info.version,
                    sdk_version: info.sdk_version,
                    build_info: info.build_info,
                    sequence: info.sequence,
                    path: info.path,
                    verified: info.verified,
                    leases,
                    quiescent: retired && leases == 0,
                    retired,
                    closed,
                    group: group.clone(),
                    connection: (process != 0).then_some(process),
                    components: info.components.into_iter().map(component_status).collect(),
                }
            })
            .collect::<Vec<_>>();
        libraries.sort_by_key(|library| library.id);
        payload::LibraryStatus {
            libraries,
            retired_images: lease::retired_images(),
            retirement: Some(lease::retirement_status(true)),
        }
    }

    fn activate(&self, id: u64) -> Result<()> {
        let name = self
            .0
            .libraries
            .lock()
            .get(&id)
            .map(|library| library.name().to_owned())
            .ok_or_else(|| Error::new(4, "原生库编号已退役"))?;
        self.0.active.lock().insert(name, id);
        Ok(())
    }

    /// DOC-11 SW-6、ADR-0012：核心已完成实例清理，等待库租约归零后按策略释放句柄。
    async fn retire(&self, id: u64) -> Result<payload::LibraryRetired> {
        let library = {
            let mut libraries = self.0.libraries.lock();
            let mut retired = self.0.retired.lock();
            let library = libraries
                .remove(&id)
                .ok_or_else(|| Error::new(4, "原生库编号已退役"))?;
            retired.insert(
                id,
                RetiredLibrary {
                    info: library.info(),
                    lease: library.lease.clone(),
                },
            );
            library
        };
        self.0.active.lock().retain(|_, active| *active != id);
        let started = Instant::now();
        let lease = library.retire();
        lease.quiet().await;
        lease.reap();
        lease::reap_ready();
        if lease.closed() {
            self.trim_closed_history();
        }
        self.prune_cache().await;
        lease.descriptor_result().map_err(|problem| {
            Error::new(
                problem.code(),
                format!("库 {id} 的描述符释放失败：{problem}"),
            )
            .with_detail(problem.detail().to_vec())
        })?;
        Ok(payload::LibraryRetired {
            leases: lease.count() as u64,
            closed: lease.closed(),
            quiescence_ms: Some(started.elapsed().as_millis() as u64),
            retired_images: lease::retired_images(),
            retirement: Some(lease::retirement_status(false)),
        })
    }

    fn trim_closed_history(&self) {
        const CLOSED_HISTORY: usize = 256;
        let mut retired = self.0.retired.lock();
        if retired.len() <= CLOSED_HISTORY {
            return;
        }
        let mut closed: Vec<_> = retired
            .iter()
            .filter_map(|(id, entry)| entry.lease.closed().then_some(*id))
            .collect();
        closed.sort_unstable();
        let discard = closed.len().saturating_sub(CLOSED_HISTORY);
        for id in closed.into_iter().take(discard) {
            retired.remove(&id);
        }
    }

    async fn request(&self, method: &str, params: impl Into<Outgoing>) -> Result<Payload> {
        self.connection()?
            .request(method, params, self.0.options.request_timeout)
            .await
            .map_err(from_wire)
    }

    fn request_sync(&self, method: &str, params: impl Into<Outgoing>) -> Result<Payload> {
        self.0.executor.wait(self.request(method, params))
    }

    /// ADR-0046：异步宿主操作在拥有执行器上运行；查询可取消，已受理的生命周期操作继续完成。
    fn scheduled<F, T>(&self, future: F, cancellable: bool) -> Result<RemoteFuture<T>>
    where
        F: std::future::Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        make_future(
            self.0.executor.start(future, cancellable),
            abi::environment(),
        )
    }

    /// DOC-15 §6.4：等待通知任务结束后移除本地视图，并共享同一关闭结果。
    async fn close_context(&self, id: u64, record: Arc<ContextRecord>) -> Result<()> {
        let drain = record.phase.send_if_modified(|phase| {
            if matches!(phase, ClosePhase::Open | ClosePhase::Closing) {
                *phase = ClosePhase::Draining;
                return true;
            }
            false
        });
        if !drain {
            let mut phase = record.phase.subscribe();
            loop {
                if let ClosePhase::Closed(result) = &*phase.borrow_and_update() {
                    return result.clone();
                }
                phase
                    .changed()
                    .await
                    .map_err(|error| Error::new(4, error.to_string()))?;
            }
        }
        let tasks = std::mem::take(&mut *record.notifications.lock());
        for task in tasks.values() {
            if let Some(cancel) = &task.cancel {
                cancel.cancel();
            }
        }
        let mut failure = None;
        for (_, task) in tasks {
            if let Err(error) = task.done.await {
                failure.get_or_insert_with(|| Error::new(5, error.to_string()));
            }
        }
        let removed = self.0.contexts.lock().remove(&id);
        drop(removed);
        let result = failure.map_or(Ok(()), Err);
        record
            .phase
            .send_replace(ClosePhase::Closed(result.clone()));
        result
    }

    /// DOC-15 Q0：核心完成组件清理后，关闭视图、释放对象并等待原生库静止。
    async fn shutdown(&self) -> Result<()> {
        let records: Vec<_> = self
            .0
            .contexts
            .lock()
            .iter()
            .map(|(id, record)| (*id, record.clone()))
            .collect();
        let mut failure = None;
        for (id, record) in records {
            if let Err(error) = self.close_context(id, record).await {
                failure.get_or_insert(error);
            }
        }
        self.objects().clear();
        let libraries = self.0.libraries.lock().keys().copied().collect::<Vec<_>>();
        for library in libraries {
            if let Err(error) = self.retire(library).await {
                failure.get_or_insert(error);
            }
        }
        self.0.store.release_sources();
        self.prune_cache().await;
        self.0.stopped.store(true, Ordering::Release);
        failure.map_or(Ok(()), Err)
    }

    /// ADR-0053：租约归还后的目录回收独立执行，回收错误保留诊断。
    async fn prune_cache(&self) {
        if let Err(error) = cordis_process::cache::prune_directories(
            vec![self.0.store.directory().to_owned()],
            self.0.options.request_timeout,
        )
        .await
        {
            eprintln!("库副本回收失败：{error}");
        }
    }
}

/// DOC-16：将插件组件描述转换为管理状态公开的数据类型。
fn component_status(component: payload::Component) -> cordis_types::ComponentInfo {
    cordis_types::ComponentInfo {
        name: component.name,
        inject: component
            .inject
            .into_iter()
            .map(|dependency| cordis_types::DependencyInfo {
                interface: dependency.interface,
                required: dependency.required,
            })
            .collect(),
        provides: component.provides,
        listens: component.listens,
        state_schema: component.state_schema,
        state_version_min: component.state_version_min,
        state_version_max: component.state_version_max,
    }
}
