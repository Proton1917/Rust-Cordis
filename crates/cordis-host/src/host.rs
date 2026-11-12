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

#[derive(Clone, Debug)]
pub struct HostOptions {
    pub libraries: Vec<PathBuf>,
    pub library: LibraryOptions,
    pub request_timeout: Duration,
    pub native_workers: usize,
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
    closing: AtomicBool,
    close_started: AtomicBool,
    closed: watch::Sender<Option<Result<()>>>,
}

impl ContextRecord {
    fn new(data: Arc<ContextData>) -> Self {
        Self {
            data,
            notifications: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            close_started: AtomicBool::new(false),
            closed: watch::channel(None).0,
        }
    }
}

struct RetiredLibrary {
    info: crate::library::LibraryInfo,
    lease: Arc<lease::Lease>,
}

pub(crate) struct Inner {
    options: HostOptions,
    store: LibraryStore,
    libraries: Mutex<HashMap<u64, NativeLibrary>>,
    retired: Mutex<HashMap<u64, RetiredLibrary>>,
    active: Mutex<HashMap<String, u64>>,
    connection: OnceLock<Connection>,
    process: AtomicU64,
    group: OnceLock<String>,
    next_notification: AtomicU64,
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
    /// DOC-15 CP-1：初始化宿主数据和独立原生执行器，运行时构造错误直接返回。
    pub fn new(options: HostOptions, io: tokio::runtime::Handle) -> Result<Self> {
        let store = LibraryStore::new(options.library.clone())?;
        let native = RuntimeOwner::new(options.native_workers)?;
        let executor = NativeExecutor::new(native.handle(), io.clone());
        Ok(Self(Arc::new(Inner {
            options,
            store,
            libraries: Mutex::new(HashMap::new()),
            retired: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            connection: OnceLock::new(),
            process: AtomicU64::new(0),
            group: OnceLock::new(),
            next_notification: AtomicU64::new(1),
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

    fn connection(&self) -> Result<Connection> {
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
                    verified: false,
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
        Ok(payload::LibraryRetired {
            leases: lease.count() as u64,
            closed: lease.closed(),
            quiescence_ms: Some(started.elapsed().as_millis() as u64),
            retired_images: lease::retired_images(),
        })
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

    pub fn log(&self, context: u64, level: u32, message: &str) -> Result<()> {
        if context != 0 {
            self.context(context)?;
        }
        self.request_sync(
            "log",
            to_value(&payload::Log {
                context,
                level,
                message: message.into(),
            })?,
        )?;
        Ok(())
    }

    pub fn now(&self, context: u64) -> Result<u64> {
        self.context(context)?;
        Ok(self.0.started.elapsed().as_nanos() as u64)
    }

    pub fn sleep(&self, context: u64, nanos: u64) -> Result<RemoteFuture<AbiUnit>> {
        self.context(context)?;
        make_future(
            async move {
                tokio::time::sleep(Duration::from_nanos(nanos)).await;
                Ok(AbiUnit::default())
            },
            abi::environment(),
        )
    }

    pub fn is_current(&self, context: u64) -> Result<bool> {
        let response = self.request_sync("is_current", to_value(&payload::Context { context })?)?;
        from_value(response.value().clone())
    }

    pub fn config(&self, context: u64) -> Result<Vec<u8>> {
        Ok(self.context(context)?.data.config.clone())
    }
    pub fn inherited_state(&self, context: u64) -> Result<Option<State>> {
        Ok(self.context(context)?.data.inherited.clone())
    }

    /// ADR-0025：核心返回拥有租约的视图，共享其加载配置并与退役登记同步。
    pub fn context_derive(
        &self,
        context: u64,
        options: &[u8],
    ) -> Result<cordis_abi::DerivedContext> {
        let parent = self.context(context)?;
        let options: payload::ContextOptions = from_value(decode_value(options)?)?;
        let request = payload::ContextDerive {
            context,
            isolate: options.isolate,
            intercept: options.intercept,
        };
        let response = self.request_sync("context_derive", to_value(&request)?)?;
        let result: payload::ContextDerived = from_value(response.value().clone())?;
        let reference = response.import(result.object).map_err(from_wire)?;
        let lease = self.bridge()?.import(
            reference,
            payload::ObjectInfo {
                interface: "cordis.context-view@1.0".into(),
                methods: Vec::new(),
                local: None,
            },
        )?;
        {
            let mut contexts = self.0.contexts.lock();
            if parent.closing.load(Ordering::Acquire)
                || parent.data.retiring.load(Ordering::Acquire)
            {
                return Err(Error::new(4, "父上下文正在释放"));
            }
            if let Some(existing) = contexts.get(&result.context) {
                if existing.data.base != parent.data.base {
                    return Err(Error::new(2, "派生视图属于其他加载纪元"));
                }
            } else {
                contexts.insert(
                    result.context,
                    Arc::new(ContextRecord::new(parent.data.clone())),
                );
            }
        }
        Ok(cordis_abi::DerivedContext {
            context: result.context,
            lease,
        })
    }

    /// ADR-0025：子组件选择和装入交由核心，Future 保留请求至组件稳定。
    pub fn plugin(
        &self,
        context: u64,
        component: &str,
        config: &[u8],
    ) -> Result<RemoteFuture<u64>> {
        let host = self.clone();
        let request = payload::PluginRequest {
            context,
            component: component.into(),
            config: decode_value(config)?,
        };
        make_future(
            async move {
                let response = host.request("plugin", to_value(&request)?).await?;
                Ok(from_value::<payload::PluginResult>(response.value().clone())?.instance)
            },
            abi::environment(),
        )
    }

    /// DOC-12 §9：效应回调作为拥有资源发送，核心登记成功后返回外部句柄。
    pub fn effect(
        &self,
        context: u64,
        label: &str,
        body: Object,
        asynchronous: bool,
    ) -> Result<u64> {
        let mut refs = self.objects().export_batch(abi::environment());
        let body = refs.insert(body);
        let params = payload::Effect {
            context,
            label: label.into(),
            body,
            asynchronous,
            refs: refs.refs().to_vec(),
        };
        let outgoing = Outgoing::with_commit(to_value(&params)?, move || refs.commit());
        let response = self.request_sync("effect", outgoing)?;
        Ok(from_value::<payload::EffectHandle>(response.value().clone())?.effect)
    }

    pub fn defer(&self, scope: u64, cleanup: Object, asynchronous: bool) -> Result<()> {
        let mut refs = self.objects().export_batch(abi::environment());
        let callback = refs.insert(cleanup);
        let params = payload::Defer {
            scope,
            callback,
            asynchronous,
            refs: refs.refs().to_vec(),
        };
        self.request_sync(
            "defer",
            Outgoing::with_commit(to_value(&params)?, move || refs.commit()),
        )?;
        Ok(())
    }

    pub fn effect_wait(&self, effect: u64, dispose: bool) -> Result<RemoteFuture<AbiUnit>> {
        let host = self.clone();
        make_future(
            async move {
                host.request(
                    if dispose {
                        "effect_dispose"
                    } else {
                        "effect_settled"
                    },
                    to_value(&payload::EffectHandle { effect })?,
                )
                .await?;
                Ok(AbiUnit::default())
            },
            abi::environment(),
        )
    }

    /// DOC-12 §9：最终 SDK 句柄释放经有界通知发送，发送失败终止失去一致性的连接。
    pub fn effect_release(&self, effect: u64) -> Result<()> {
        let connection = self.connection()?;
        if let Err(error) = connection.try_notify(
            "effect_release",
            to_value(&payload::EffectHandle { effect })?,
        ) {
            connection.terminate("效应句柄释放通知发送失败");
            return Err(from_wire(error));
        }
        Ok(())
    }

    pub fn provide(
        &self,
        context: u64,
        interface: &str,
        value: Object,
        scoped: bool,
    ) -> Result<()> {
        let methods = value.method_names()?;
        let mut refs = self.objects().export_batch(abi::environment());
        let object = refs.insert(value);
        let params = payload::Provide {
            context,
            interface: interface.into(),
            object,
            methods,
            scoped,
            refs: refs.refs().to_vec(),
        };
        self.request_sync(
            "provide",
            Outgoing::with_commit(to_value(&params)?, move || refs.commit()),
        )?;
        Ok(())
    }

    /// DOC-15 §8：服务授权与解析经核心，同组结果恢复本地 ABI 对象。
    pub fn get(&self, context: u64, interface: &str) -> Result<Object> {
        let response = self.request_sync(
            "get",
            to_value(&payload::Get {
                context,
                interface: interface.into(),
            })?,
        )?;
        let result: payload::GetResult = from_value(response.value().clone())?;
        let object = response.import(result.object).map_err(from_wire)?;
        self.bridge()?.import(
            object,
            payload::ObjectInfo {
                interface: result.interface,
                methods: result.methods,
                local: result.local,
            },
        )
    }

    pub fn on(&self, context: u64, event: &str, listener: Object) -> Result<()> {
        let asynchronous = match listener.header().kind {
            2 => false,
            3 => true,
            _ => return Err(Error::new(2, "事件监听器类型无效")),
        };
        let mut refs = self.objects().export_batch(abi::environment());
        let callback = refs.insert(listener);
        let params = payload::On {
            context,
            event: event.into(),
            callback,
            asynchronous,
            refs: refs.refs().to_vec(),
        };
        self.request_sync(
            "on",
            Outgoing::with_commit(to_value(&params)?, move || refs.commit()),
        )?;
        Ok(())
    }

    fn emit_message(
        &self,
        context: u64,
        event: &str,
        bytes: &[u8],
        asynchronous: bool,
    ) -> Result<Outgoing> {
        let mut payload = decode_value(bytes)?;
        let mut refs = self.objects().export_batch(abi::environment());
        refs.rewrite(&mut payload)?;
        let params = payload::Emit {
            context,
            event: event.into(),
            payload,
            asynchronous,
            refs: refs.refs().to_vec(),
        };
        Ok(Outgoing::with_commit(to_value(&params)?, move || {
            refs.commit()
        }))
    }

    pub fn emit(&self, context: u64, event: &str, bytes: &[u8], asynchronous: bool) -> Result<()> {
        self.request_sync(
            "emit",
            self.emit_message(context, event, bytes, asynchronous)?,
        )?;
        Ok(())
    }

    pub fn emit_async(
        &self,
        context: u64,
        event: &str,
        bytes: &[u8],
    ) -> Result<RemoteFuture<AbiUnit>> {
        let message = self.emit_message(context, event, bytes, true)?;
        let host = self.clone();
        make_future(
            async move {
                host.request("emit", message).await?;
                Ok(AbiUnit::default())
            },
            abi::environment(),
        )
    }

    pub fn export_state(
        &self,
        context: u64,
        schema: &str,
        version: u32,
        exporter: Object,
    ) -> Result<()> {
        let mut refs = self.objects().export_batch(abi::environment());
        let exporter = refs.insert(exporter);
        let params = payload::ExportState {
            context,
            schema: schema.into(),
            version,
            exporter,
            refs: refs.refs().to_vec(),
        };
        self.request_sync(
            "export_state",
            Outgoing::with_commit(to_value(&params)?, move || refs.commit()),
        )?;
        Ok(())
    }

    async fn callback(
        &self,
        request: payload::Callback,
        payload: Payload,
        object: Arc<Owned<Object>>,
    ) -> Result<()> {
        let bytes = self.bridge()?.incoming(&payload, request.payload).await?;
        let result = match object.header().kind {
            2 => object.call_sync(request.context, request.scope, bytes.as_slice()),
            3 => object
                .call_async(request.context, request.scope, bytes.as_slice())?
                .await
                .map(|_| ()),
            _ => Err(Error::new(2, "对象对应服务或导出器，回调入口无效")),
        };
        bytes.release()?;
        result
    }

    /// DOC-15 §6.4：等待通知任务结束后移除本地视图，并共享同一关闭结果。
    async fn close_context(&self, id: u64, record: Arc<ContextRecord>) -> Result<()> {
        record.closing.store(true, Ordering::Release);
        if record.close_started.swap(true, Ordering::AcqRel) {
            let mut closed = record.closed.subscribe();
            loop {
                if let Some(result) = closed.borrow_and_update().clone() {
                    return result;
                }
                closed
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
        record.closed.send_replace(Some(result.clone()));
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
        for (id, record) in records {
            self.close_context(id, record).await?;
        }
        self.objects().clear();
        let libraries = self.0.libraries.lock().keys().copied().collect::<Vec<_>>();
        for library in libraries {
            self.retire(library).await?;
        }
        self.0.stopped.store(true, Ordering::Release);
        Ok(())
    }

    /// DOC-16：握手建立进程身份并装入核心指定的初始原生库。
    fn hello(&self, request: payload::Hello) -> Result<Value> {
        if request.protocol != 1 {
            return Err(Error::new(3, "UP/1 主版本不匹配"));
        }
        if self.0.initialized.swap(true, Ordering::AcqRel) {
            return Err(Error::new(4, "宿主握手已执行"));
        }
        self.0.process.store(request.process, Ordering::Release);
        let _group = self.0.group.set(request.group);
        let mut plugins = Vec::new();
        for path in &self.0.options.libraries {
            let prepared = self.prepare(path)?;
            if self.0.active.lock().contains_key(&prepared.plugin.name) {
                return Err(Error::new(2, "启动列表包含同名插件库"));
            }
            self.activate(prepared.library)?;
            plugins.push(prepared.plugin);
        }
        to_value(&payload::Describe {
            protocol: 1,
            plugins,
        })
    }

    /// DOC-12 §10、DOC-15 Q0：为核心分配的加载纪元保留配置与描述符，再轮询 apply。
    async fn apply(&self, request: payload::Apply) -> Result<Value> {
        let component = self.component(&request.plugin, &request.component, request.library)?;
        let config = encode(&request.config)?;
        let inherited = request
            .inherited
            .map(|state| {
                Ok::<_, Error>(State {
                    schema: state.schema,
                    version: state.version,
                    data: encode(&state.data)?,
                })
            })
            .transpose()?;
        let record = Arc::new(ContextRecord::new(Arc::new(ContextData {
            base: request.context,
            retiring: AtomicBool::new(false),
            _component: self.0.executor.own(component.clone()),
            config: config.clone(),
            inherited,
        })));
        {
            let mut contexts = self.0.contexts.lock();
            if contexts.contains_key(&request.context) {
                return Err(Error::new(2, "上下文编号重复"));
            }
            contexts.insert(request.context, record);
        }
        component.apply(request.context, &config)?.await?;
        Ok(Value::Null)
    }

    /// DOC-16 资源协议：参数导入和结果导出分别持有调用所需资源。
    async fn invoke(&self, params: Payload) -> Result<Outgoing> {
        let request: payload::Invoke = from_value(params.value().clone())?;
        let object = self.objects().borrow(request.object)?;
        let bridge = self.bridge()?;
        let arguments = bridge.incoming(&params, Value::Array(request.args)).await?;
        let future = object.invoke_async(&request.method, arguments.as_slice())?;
        arguments.release()?;
        let result = future.await?;
        let response = bridge.outgoing(decode_value(result.as_slice())?)?;
        result.release()?;
        Ok(response)
    }

    /// DOC-16：返回原生对象的接口和同组直接调用标识。
    fn object_info(&self, request: payload::ObjectInfoRequest) -> Result<Value> {
        let info = self.objects().info(request.object)?;
        to_value(&payload::ObjectInfo {
            interface: info.interface,
            methods: info.methods,
            local: Some(payload::LocalObject {
                process: self.0.process.load(Ordering::Acquire),
                object: request.object,
            }),
        })
    }

    /// DOC-12 §9：释放导出器缓冲区后，将状态值返回核心。
    fn export(&self, request: payload::Export) -> Result<Value> {
        let result = self.objects().export_state(request.object)?;
        let value = decode_value(result.as_slice())?;
        result.release()?;
        to_value(&payload::ValueResult { value })
    }

    /// DOC-15 Q0、DOC-16：按协议方法转发核心已作出的生命周期与调用决定。
    async fn execute(&self, method: String, params: Payload) -> Result<Outgoing> {
        let reply = match method.as_str() {
            "hello" => self.hello(from_value(params.value().clone())?)?,
            "resolve" => {
                let request: payload::Resolve = from_value(params.value().clone())?;
                self.component(&request.plugin, &request.component, request.library)?
                    .resolve(&encode(&request.config)?)?;
                Value::Null
            }
            "apply" => self.apply(from_value(params.value().clone())?).await?,
            "callback" => {
                let request: payload::Callback = from_value(params.value().clone())?;
                let object = self.objects().borrow(request.object)?;
                self.callback(request, params, object).await?;
                return Ok(Value::Null.into());
            }
            "invoke" => return self.invoke(params).await,
            "object_info" => self.object_info(from_value(params.value().clone())?)?,
            "export" => self.export(from_value(params.value().clone())?)?,
            "thaw" => {
                let request: payload::Thaw = from_value(params.value().clone())?;
                self.objects().thaw(request.object)?;
                Value::Null
            }
            "library_prepare" => {
                let request: payload::LibraryPrepare = from_value(params.value().clone())?;
                to_value(&self.prepare(Path::new(&request.path))?)?
            }
            "library_status" => to_value(&self.library_status())?,
            "library_activate" => {
                let request: payload::LibraryHandle = from_value(params.value().clone())?;
                self.activate(request.library)?;
                Value::Null
            }
            "library_retire" => {
                let request: payload::LibraryHandle = from_value(params.value().clone())?;
                to_value(&self.retire(request.library).await?)?
            }
            "shutdown" => {
                self.shutdown().await?;
                Value::Null
            }
            _ => return Err(Error::new(3, format!("宿主请求方法 {method} 无对应入口"))),
        };
        Ok(reply.into())
    }

    /// DOC-16：通知回调在原生执行器运行，关闭上下文时取消并等待该批任务。
    fn notify_callback(&self, params: Payload) -> Result<()> {
        let request: payload::Callback = from_value(params.value().clone())?;
        let record = {
            let contexts = self.0.contexts.lock();
            match contexts.get(&request.context) {
                Some(record) => record.clone(),
                _ => return Ok(()),
            }
        };
        let (id, task, done) = {
            let mut notifications = record.notifications.lock();
            if record.closing.load(Ordering::Acquire) || self.0.stopping.load(Ordering::Acquire) {
                return Ok(());
            }
            let object = self.objects().borrow(request.object)?;
            let host = self.clone();
            let task = self.0.executor.start(
                async move { host.callback(request, params, object).await },
                true,
            );
            let cancel = task.cancellation();
            let id = self.0.next_notification.fetch_add(1, Ordering::Relaxed);
            let (done, receiver) = tokio::sync::oneshot::channel();
            notifications.insert(
                id,
                NotificationTask {
                    cancel,
                    done: receiver,
                },
            );
            (id, task, done)
        };
        let host = self.clone();
        self.0.runtime.spawn(async move {
            let result = task.await;
            record.notifications.lock().remove(&id);
            let _receiver = done.send(());
            if let Err(error) = result
                && error.code() != 7
            {
                abi::environment().report(error);
            }
            drop(host);
        });
        Ok(())
    }

    /// ADR-0025：释放基上下文时同时退役其全部派生视图。
    fn release_context(&self, context: u64) {
        let records = {
            let contexts = self.0.contexts.lock();
            match contexts.get(&context) {
                Some(record) if record.data.base == context => {
                    record.data.retiring.store(true, Ordering::Release);
                    contexts
                        .iter()
                        .filter(|(_, candidate)| candidate.data.base == context)
                        .map(|(id, record)| {
                            record.closing.store(true, Ordering::Release);
                            (*id, record.clone())
                        })
                        .collect::<Vec<_>>()
                }
                Some(record) => {
                    record.closing.store(true, Ordering::Release);
                    vec![(context, record.clone())]
                }
                None => Vec::new(),
            }
        };
        for (id, record) in records {
            record.closing.store(true, Ordering::Release);
            let host = self.clone();
            let task = self
                .0
                .executor
                .start(async move { host.close_context(id, record).await }, false);
            self.0.runtime.spawn(async move {
                if let Err(error) = task.await {
                    abi::environment().report(error);
                }
            });
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

impl Handler for Host {
    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, std::result::Result<Outgoing, WireError>> {
        self.0.connection.get_or_init(|| peer);
        if method == "shutdown" {
            self.0.stopping.store(true, Ordering::Release);
        }
        let host = self.clone();
        let cancellable = method == "invoke";
        Box::pin(async move {
            host.0
                .executor
                .clone()
                .run(
                    async move { host.execute(method, params).await },
                    cancellable,
                )
                .await
                .map_err(to_wire)
        })
    }

    fn notify(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, std::result::Result<(), WireError>> {
        self.0.connection.get_or_init(|| peer);
        Box::pin(async move {
            match method.as_str() {
                "callback" => self.notify_callback(params).map_err(to_wire)?,
                "release" => {
                    let request: payload::Release =
                        from_value(params.value().clone()).map_err(to_wire)?;
                    for id in request.objects {
                        self.objects().release(id).map_err(to_wire)?;
                    }
                }
                "release_context" => {
                    let request: payload::Context =
                        from_value(params.value().clone()).map_err(to_wire)?;
                    self.release_context(request.context);
                }
                _ => return Err(WireError::new(3, format!("宿主通知 {method} 无对应入口"))),
            }
            Ok(())
        })
    }
}
