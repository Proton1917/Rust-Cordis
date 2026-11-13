//! DOC-16 §5、PR-1：窗口的服务、效应与导出器登记到唯一核心，编号表按 release 回收。

use crate::{
    Result, error,
    ui::{Bridge, Signal, WindowLease, WindowSpec},
};
use cordis_up::{
    Value, WireError, payload,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload, RemoteObject},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, mpsc};

const TIMEOUT: Duration = Duration::from_secs(30);
const INTERFACE: &str = "window@1.0";
const SCHEMA: &str = "window.snapshot";

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    title: String,
    width: u32,
    height: u32,
    quit_on_last_window: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            title: "Cordis".into(),
            width: 1000,
            height: 720,
            quit_on_last_window: true,
        }
    }
}

struct App {
    context: u64,
    peer: Connection,
    manage: RemoteObject,
    bridge: Bridge,
    quit_on_last_window: bool,
    effects: Mutex<BTreeMap<u64, u64>>,
}

struct Plan {
    app: Arc<App>,
    id: u64,
    spec: WindowSpec,
}

enum Resource {
    Service(Arc<App>),
    Body(Arc<Plan>),
    Cleanup(Arc<WindowLease>),
    Exporter(Arc<App>),
}

struct Shared {
    next: AtomicU64,
    objects: Mutex<BTreeMap<u64, Arc<Resource>>>,
    apps: Mutex<BTreeMap<u64, Arc<App>>>,
    bridge: Bridge,
    stopped: Notify,
}

struct Offer {
    shared: Weak<Shared>,
    id: u64,
    committed: bool,
}
impl Offer {
    fn outgoing(self, value: Value) -> Outgoing {
        Outgoing::with_commit(value, move || self.commit())
    }
    fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for Offer {
    fn drop(&mut self) {
        if !self.committed
            && let Some(shared) = self.shared.upgrade()
        {
            let released = shared.objects.lock().remove(&self.id);
            drop(released);
        }
    }
}

fn value<T: Serialize>(data: &T) -> Result<Value> {
    Value::serialized(data).map_err(error)
}
fn decode<T: serde::de::DeserializeOwned>(data: &Value) -> Result<T> {
    data.deserialized().map_err(error)
}
fn methods() -> Vec<String> {
    ["open", "close"].map(str::to_owned).to_vec()
}

/// DOC-15 CP-10：输入关闭、核心 shutdown 或界面循环结束都会回收协议执行器。
pub async fn run(bridge: Bridge, mut signals: mpsc::Receiver<Signal>) -> Result<()> {
    let shared = Arc::new(Shared {
        next: AtomicU64::new(1),
        objects: Mutex::new(BTreeMap::new()),
        apps: Mutex::new(BTreeMap::new()),
        bridge,
        stopped: Notify::new(),
    });
    let peer = Connection::attach(
        0,
        tokio::io::stdin(),
        tokio::io::stdout(),
        ConnectionOptions::default(),
        Arc::new(Plugin(shared.clone())),
    )?;
    loop {
        tokio::select! {
            _ = peer.closed() => break,
            _ = shared.stopped.notified() => break,
            signal = signals.recv() => match signal {
                Some(signal) => {
                    if let Err(problem) = shared.clone().signal(signal).await {
                        eprintln!("窗口操作失败：{problem}");
                    }
                }
                None => break,
            },
        }
    }
    Ok(())
}

impl Shared {
    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
    fn offer(self: &Arc<Self>, resource: Resource) -> (u64, Offer) {
        let id = self.id();
        self.objects.lock().insert(id, Arc::new(resource));
        (
            id,
            Offer {
                shared: Arc::downgrade(self),
                id,
                committed: false,
            },
        )
    }
    fn object(&self, id: u64) -> Result<Arc<Resource>> {
        self.objects
            .lock()
            .get(&id)
            .cloned()
            .ok_or_else(|| WireError::new(4, "窗口资源编号已释放"))
    }

    /// DOC-15 §11.6：Web 提供页面地址，管理授权用于用户触发的有序退出。
    async fn apply(self: &Arc<Self>, peer: Connection, request: payload::Apply) -> Result<()> {
        let config: Config = decode(&request.config)?;
        let manage = get(&peer, request.context, "cordis.manage@1.0").await?;
        let web = get(&peer, request.context, "cordis.web@1.0").await?;
        #[derive(Deserialize)]
        struct Entry {
            url: String,
        }
        let entry: Entry = decode(&invoke(&web, "entry", Vec::new()).await?)?;
        let app = Arc::new(App {
            context: request.context,
            peer,
            manage,
            bridge: self.bridge.clone(),
            quit_on_last_window: config.quit_on_last_window,
            effects: Mutex::new(BTreeMap::new()),
        });
        app.bridge.register(app.context).await?;
        self.apps.lock().insert(app.context, app.clone());
        self.exporter(app.clone()).await?;
        let windows: Vec<WindowSpec> = match request.inherited {
            Some(state) if state.schema == SCHEMA && state.version == 1 => decode(&state.data)?,
            Some(_) => return Err(error("窗口继承状态的模式或版本与声明不符")),
            None => vec![WindowSpec {
                url: entry.url.clone(),
                title: config.title,
                width: config.width,
                height: config.height,
                entry: true,
            }],
        };
        for mut spec in windows {
            if spec.entry {
                spec.url.clone_from(&entry.url);
            }
            self.open(app.clone(), spec).await?;
        }
        let (object, offer) = self.offer(Resource::Service(app.clone()));
        app.peer
            .request(
                "provide",
                offer.outgoing(value(&payload::Provide {
                    context: app.context,
                    interface: INTERFACE.into(),
                    object,
                    methods: methods(),
                    scoped: false,
                    refs: vec![object],
                })?),
                TIMEOUT,
            )
            .await?;
        Ok(())
    }

    async fn exporter(self: &Arc<Self>, app: Arc<App>) -> Result<()> {
        let (exporter, offer) = self.offer(Resource::Exporter(app.clone()));
        app.peer
            .request(
                "export_state",
                offer.outgoing(value(&payload::ExportState {
                    context: app.context,
                    schema: SCHEMA.into(),
                    version: 1,
                    exporter,
                    refs: vec![exporter],
                })?),
                TIMEOUT,
            )
            .await?;
        Ok(())
    }

    /// PR-1：核心先完成窗口效应及逆变换登记，随后窗口显示并返回编号。
    async fn open(self: &Arc<Self>, app: Arc<App>, spec: WindowSpec) -> Result<u64> {
        dimensions(spec.width, spec.height)?;
        let id = self.id();
        let (body, offer) = self.offer(Resource::Body(Arc::new(Plan {
            app: app.clone(),
            id,
            spec,
        })));
        let reply = app
            .peer
            .request(
                "effect",
                offer.outgoing(value(&payload::Effect {
                    context: app.context,
                    label: format!("window:{id}"),
                    body,
                    asynchronous: false,
                    refs: vec![body],
                })?),
                TIMEOUT,
            )
            .await?;
        let effect: payload::EffectHandle = decode(reply.value())?;
        app.effects.lock().insert(id, effect.effect);
        if let Err(problem) = app.bridge.show(id).await {
            app.close(id, false).await?;
            return Err(problem);
        }
        Ok(id)
    }

    /// DOC-16 §5、PR-1：效应体登记窗口逆变换，清理等待主线程实际释放窗口。
    async fn callback(self: &Arc<Self>, request: payload::Callback) -> Result<()> {
        let resource = self.object(request.object)?;
        match resource.as_ref() {
            Resource::Body(plan) => {
                let lease = Arc::new(
                    plan.app
                        .bridge
                        .open(plan.app.context, plan.id, plan.spec.clone())
                        .await?,
                );
                let (callback, offer) = self.offer(Resource::Cleanup(lease));
                plan.app
                    .peer
                    .request(
                        "defer",
                        offer.outgoing(value(&payload::Defer {
                            scope: request.scope,
                            callback,
                            asynchronous: true,
                            refs: vec![callback],
                        })?),
                        TIMEOUT,
                    )
                    .await?;
                Ok(())
            }
            Resource::Cleanup(lease) => self.bridge.close(lease.id).await,
            _ => Err(error("窗口资源的用途与 callback 不符")),
        }
    }

    /// DOC-15 §11.6：用户关闭释放对应效应，应用退出经已注入的管理服务请求。
    async fn signal(self: Arc<Self>, signal: Signal) -> Result<()> {
        match signal {
            Signal::Quit => {
                let app = self.apps.lock().values().next().cloned();
                if let Some(app) = app {
                    invoke(&app.manage, "shutdown", Vec::new()).await?;
                }
            }
            Signal::Closed {
                owner,
                window,
                last,
            } => {
                let app = self.apps.lock().get(&owner).cloned();
                if let Some(app) = app {
                    app.close(window, true).await?;
                    if last && app.quit_on_last_window {
                        invoke(&app.manage, "shutdown", Vec::new()).await?;
                    }
                }
            }
        }
        Ok(())
    }

    /// DOC-16 §7：参数按接口顺序解码，窗口结果由主线程确认，未知方法返回 code 3。
    async fn invoke(self: &Arc<Self>, request: payload::Invoke) -> Result<Value> {
        let resource = self.object(request.object)?;
        let Resource::Service(app) = resource.as_ref() else {
            return Err(error("窗口服务对象类型不符"));
        };
        match request.method.as_str() {
            "open" => {
                let (url, title, width, height): (String, String, u32, u32) =
                    decode(&Value::Array(request.args))?;
                value(
                    &self
                        .open(
                            app.clone(),
                            WindowSpec {
                                url,
                                title,
                                width,
                                height,
                                entry: false,
                            },
                        )
                        .await?,
                )
            }
            "close" => {
                let (id,): (u64,) = decode(&Value::Array(request.args))?;
                let last = app.bridge.user_close(id).await?;
                app.close(id, true).await?;
                if last && app.quit_on_last_window {
                    invoke(&app.manage, "shutdown", Vec::new()).await?;
                }
                Ok(Value::Null)
            }
            _ => Err(WireError::new(3, "窗口方法缺席")),
        }
    }

    fn object_info(&self, request: payload::ObjectInfoRequest) -> Result<Value> {
        if !matches!(self.object(request.object)?.as_ref(), Resource::Service(_)) {
            return Err(error("窗口服务对象类型不符"));
        }
        value(&payload::ObjectInfo {
            interface: INTERFACE.into(),
            methods: methods(),
            local: None,
        })
    }

    /// CP-11、PR-7：导出与解冻交给同一 GUI 执行者，与异步窗口修改保持确定次序。
    async fn state_request(&self, method: &str, request: payload::Export) -> Result<Value> {
        let resource = self.object(request.object)?;
        let Resource::Exporter(app) = resource.as_ref() else {
            return Err(error("窗口导出器对象类型不符"));
        };
        if method == "export" {
            value(&payload::ValueResult {
                value: value(&app.bridge.snapshot(app.context).await?)?,
            })
        } else {
            app.bridge.thaw(app.context).await?;
            Ok(Value::Null)
        }
    }
}

impl App {
    async fn close(&self, id: u64, emit: bool) -> Result<()> {
        let effect = self
            .effects
            .lock()
            .remove(&id)
            .ok_or_else(|| WireError::new(4, "窗口编号已关闭"))?;
        let parameters = value(&payload::EffectHandle { effect })?;
        self.peer
            .request("effect_dispose", parameters.clone(), TIMEOUT)
            .await?;
        self.peer.notify("effect_release", parameters).await?;
        if emit {
            #[derive(Serialize)]
            struct Closed {
                id: u64,
            }
            self.peer
                .request(
                    "emit",
                    value(&payload::Emit {
                        context: self.context,
                        event: "window.closed@1.0".into(),
                        payload: value(&Closed { id })?,
                        asynchronous: true,
                        refs: Vec::new(),
                    })?,
                    TIMEOUT,
                )
                .await?;
        }
        Ok(())
    }
}

fn dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(error("窗口宽度和高度需要正整数"));
    }
    Ok(())
}

async fn get(peer: &Connection, context: u64, interface: &str) -> Result<RemoteObject> {
    let reply = peer
        .request(
            "get",
            value(&payload::Get {
                context,
                interface: interface.into(),
            })?,
            TIMEOUT,
        )
        .await?;
    let result: payload::GetResult = decode(reply.value())?;
    reply.import(result.object)
}

async fn invoke(object: &RemoteObject, method: &str, args: Vec<Value>) -> Result<Value> {
    let reply = object
        .peer()
        .request(
            "invoke",
            value(&payload::Invoke {
                object: object.id(),
                method: method.into(),
                args,
                refs: Vec::new(),
            })?,
            TIMEOUT,
        )
        .await?;
    Ok(decode::<payload::InvokeResult>(reply.value())?.value)
}

struct Plugin(Arc<Shared>);

impl Handler for Plugin {
    fn request(
        &self,
        peer: Connection,
        method: String,
        input: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing>> {
        Box::pin(async move {
            let shared = &self.0;
            let result = match method.as_str() {
                "hello" => {
                    let hello: payload::Hello = decode(input.value())?;
                    if hello.protocol != 1 {
                        return Err(WireError::new(3, "窗口插件需要 UP/1"));
                    }
                    value(&description())?
                }
                "resolve" => {
                    let resolve: payload::Resolve = decode(input.value())?;
                    let config: Config = decode(&resolve.config)?;
                    dimensions(config.width, config.height)?;
                    Value::Null
                }
                "apply" => {
                    shared.apply(peer, decode(input.value())?).await?;
                    Value::Null
                }
                "callback" => {
                    shared.callback(decode(input.value())?).await?;
                    Value::Null
                }
                "invoke" => value(&payload::InvokeResult {
                    value: shared.invoke(decode(input.value())?).await?,
                    refs: Vec::new(),
                })?,
                "object_info" => shared.object_info(decode(input.value())?)?,
                "export" | "thaw" => {
                    shared
                        .state_request(&method, decode(input.value())?)
                        .await?
                }
                "shutdown" => {
                    let shared = shared.clone();
                    return Ok(
                        Outgoing::new(Value::Null).after_write(move || shared.stopped.notify_one())
                    );
                }
                _ => return Err(WireError::new(3, "窗口 UP/1 请求方法缺席")),
            };
            Ok(Outgoing::new(result))
        })
    }

    fn notify(
        &self,
        _: Connection,
        method: String,
        input: Payload,
    ) -> HandlerFuture<'_, Result<()>> {
        Box::pin(async move {
            match method.as_str() {
                "release" => {
                    let request: payload::Release = decode(input.value())?;
                    let released = {
                        let mut objects = self.0.objects.lock();
                        request
                            .objects
                            .iter()
                            .filter_map(|id| objects.remove(id))
                            .collect::<Vec<_>>()
                    };
                    drop(released);
                }
                "release_context" => {
                    let request: payload::Context = decode(input.value())?;
                    self.0.apps.lock().remove(&request.context);
                    self.0.bridge.forget(request.context)?;
                }
                _ => return Err(WireError::new(3, "窗口 UP/1 通知方法缺席")),
            }
            Ok(())
        })
    }
}

fn description() -> payload::Describe {
    payload::Describe {
        protocol: 1,
        plugins: vec![payload::Plugin {
            name: "window".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            sdk_version: "UP/1".into(),
            build_info: "Rust system WebView process".into(),
            format: 2,
            library: None,
            components: vec![payload::Component {
                name: "window".into(),
                inject: ["cordis.web@1.0", "cordis.manage@1.0"]
                    .map(|interface| payload::Dependency {
                        interface: interface.into(),
                        required: true,
                    })
                    .to_vec(),
                provides: vec![INTERFACE.into()],
                listens: Vec::new(),
                state_schema: SCHEMA.into(),
                state_version_min: 1,
                state_version_max: 1,
            }],
        }],
    }
}
