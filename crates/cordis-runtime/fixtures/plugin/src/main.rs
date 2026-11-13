#![forbid(unsafe_code)]

use cordis_up::{
    Value, WireError, payload,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

type Result<T> = std::result::Result<T, WireError>;
fn error(problem: impl std::fmt::Display) -> WireError {
    WireError::new(1, problem.to_string())
}
fn value<T: Serialize>(value: &T) -> Result<Value> {
    Value::serialized(value).map_err(error)
}
fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    value.deserialized().map_err(error)
}

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Config {
    initial: u64,
    writes: u64,
    commands: PathBuf,
    results: PathBuf,
    observed: Option<PathBuf>,
    meta_observed: Option<PathBuf>,
    scoped: bool,
    invalid: bool,
    ready: Option<PathBuf>,
}

struct Counter {
    value: u64,
    frozen: bool,
}
struct Session {
    id: u64,
    peer: Connection,
    config: Config,
    inherited: bool,
    counter: Mutex<Counter>,
    active: AtomicBool,
    task: Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
}

enum Resource {
    Body(Arc<Session>),
    Cleanup(Arc<Session>),
    Service(Arc<Session>),
    Scoped(Arc<Session>, Value),
    Exporter(Arc<Session>),
}
struct Shared {
    role: String,
    mode: String,
    next: AtomicU64,
    objects: Mutex<HashMap<u64, Arc<Resource>>>,
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
}
struct Offer {
    shared: Weak<Shared>,
    id: u64,
    committed: bool,
}
impl Offer {
    fn commit(mut self) {
        self.committed = true;
    }
    fn outgoing(self, value: Value) -> Outgoing {
        Outgoing::with_commit(value, move || self.commit())
    }
}
impl Drop for Offer {
    fn drop(&mut self) {
        if !self.committed
            && let Some(shared) = self.shared.upgrade()
        {
            let removed = shared.objects.lock().remove(&self.id);
            drop(removed);
        }
    }
}

#[derive(Deserialize)]
struct Command {
    interface: String,
    method: String,
    args: Vec<Value>,
}
#[derive(Serialize)]
struct Reply {
    ok: bool,
    code: Option<u32>,
    value: Value,
    detail: Value,
    rpc_wall_us: u64,
}

impl Shared {
    /// ADR-0031：两个真实接口主版本用于检验联合候选之间的新依赖。
    fn counter_interface(&self) -> &'static str {
        if self.mode == "next" {
            "fixture.counter@2.0"
        } else {
            "fixture.counter@1.0"
        }
    }

    fn name(&self) -> String {
        format!("fixture-{}", self.role)
    }
    fn description(&self) -> payload::Describe {
        let inject = match self.role.as_str() {
            "admin" => vec!["cordis.manage@1.0", "cordis.runtime@1.1"],
            "reader" | "client" => vec![self.counter_interface()],
            "kv-client" => vec!["kv@1.0"],
            "native-client" => vec!["loader.counter@1.0"],
            _ => Vec::new(),
        }
        .into_iter()
        .map(|interface| payload::Dependency {
            interface: interface.into(),
            required: true,
        })
        .collect();
        payload::Describe {
            protocol: 1,
            plugins: vec![payload::Plugin {
                name: self.name(),
                version: if matches!(self.mode.as_str(), "v2" | "next") {
                    "0.2.0"
                } else {
                    "0.1.0"
                }
                .into(),
                sdk_version: "0.1.0".into(),
                build_info: "real UP1 process".into(),
                format: 2,
                library: None,
                components: vec![payload::Component {
                    name: self.role.clone(),
                    inject,
                    provides: if self.role == "store" {
                        vec![self.counter_interface().into()]
                    } else {
                        Vec::new()
                    },
                    listens: Vec::new(),
                    state_schema: if self.role == "store" {
                        "fixture.snapshot".into()
                    } else {
                        String::new()
                    },
                    state_version_min: 1,
                    state_version_max: 1,
                }],
            }],
        }
    }

    fn offer(self: &Arc<Self>, resource: Resource) -> (u64, Offer) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
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
            .ok_or_else(|| WireError::new(4, "fixture object retired"))
    }

    async fn apply(self: &Arc<Self>, peer: Connection, apply: payload::Apply) -> Result<()> {
        if self.mode == "fail" {
            return Err(WireError::new(1001, "fixture apply failed"));
        }
        let config: Config = decode(&apply.config)?;
        if self.mode == "waiting" {
            let ready = config
                .ready
                .as_ref()
                .ok_or_else(|| WireError::new(2, "fixture readiness path required"))?;
            tokio::time::timeout(Duration::from_secs(30), async {
                while !ready.exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| WireError::new(6, "fixture readiness deadline"))?;
        }
        let count = match &apply.inherited {
            Some(state) => decode(&state.data)?,
            None => config.initial,
        };
        let session = Arc::new(Session {
            id: apply.context,
            peer: peer.clone(),
            config,
            inherited: apply.inherited.is_some(),
            counter: Mutex::new(Counter {
                value: count,
                frozen: false,
            }),
            active: AtomicBool::new(true),
            task: Mutex::new(None),
        });
        self.sessions.lock().insert(session.id, session.clone());
        if self.role == "store" {
            let (object, offer) = self.offer(Resource::Service(session.clone()));
            peer.request(
                "provide",
                offer.outgoing(value(&payload::Provide {
                    context: session.id,
                    interface: self.counter_interface().into(),
                    object,
                    methods: vec!["get".into(), "set".into(), "initial".into(), "meta".into()],
                    refs: vec![object],
                    scoped: session.config.scoped,
                })?),
                Duration::from_secs(5),
            )
            .await?;
        }
        if self.role == "store" && self.mode != "stateless" {
            let (exporter, offer) = self.offer(Resource::Exporter(session.clone()));
            peer.request(
                "export_state",
                offer.outgoing(value(&payload::ExportState {
                    context: session.id,
                    schema: "fixture.snapshot".into(),
                    version: 1,
                    exporter,
                    refs: vec![exporter],
                })?),
                Duration::from_secs(5),
            )
            .await?;
        }
        let (body, offer) = self.offer(Resource::Body(session));
        let response = peer
            .request(
                "effect",
                offer.outgoing(value(&payload::Effect {
                    context: apply.context,
                    label: "fixture-task".into(),
                    body,
                    asynchronous: false,
                    refs: vec![body],
                })?),
                Duration::from_secs(5),
            )
            .await?;
        let effect: payload::EffectHandle = decode(response.value())?;
        peer.notify("effect_release", value(&effect)?).await?;
        Ok(())
    }

    async fn callback(self: &Arc<Self>, callback: payload::Callback) -> Result<()> {
        let resource = self.object(callback.object)?;
        match resource.as_ref() {
            Resource::Body(session) => {
                let (cleanup, offer) = self.offer(Resource::Cleanup(session.clone()));
                session
                    .peer
                    .request(
                        "defer",
                        offer.outgoing(value(&payload::Defer {
                            scope: callback.scope,
                            callback: cleanup,
                            asynchronous: true,
                            refs: vec![cleanup],
                        })?),
                        Duration::from_secs(5),
                    )
                    .await?;
                let shared = self.clone();
                let retained = session.clone();
                *session.task.lock() =
                    Some(tokio::spawn(async move { shared.work(retained).await }));
            }
            Resource::Cleanup(session) => {
                session.active.store(false, Ordering::Release);
                let task = session.task.lock().take();
                if let Some(task) = task {
                    task.await.map_err(error)??;
                }
            }
            _ => return Err(WireError::new(2, "fixture callback type")),
        }
        Ok(())
    }

    async fn work(&self, session: Arc<Session>) -> Result<()> {
        match self.role.as_str() {
            "store" => {
                let count = if session.inherited {
                    0
                } else {
                    session.config.writes
                };
                let mut written = 0;
                while written < count && session.active.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let mut counter = session.counter.lock();
                    if !counter.frozen {
                        counter.value += 1;
                        written += 1;
                    }
                }
                if let Some(path) = &session.config.observed {
                    publish(path, session.counter.lock().value.to_string())?;
                }
            }
            "reader" => {
                let response =
                    Self::invoke(&session, self.counter_interface(), "get", Vec::new()).await?;
                if let Some(path) = &session.config.observed {
                    publish(path, serde_json::to_vec(&response).map_err(error)?)?;
                }
                if let Some(path) = &session.config.meta_observed {
                    let meta = Self::invoke(&session, self.counter_interface(), "meta", Vec::new())
                        .await?;
                    publish(path, serde_json::to_vec(&meta).map_err(error)?)?;
                }
            }
            "admin" | "client" | "kv-client" | "native-client" => {
                fs::create_dir_all(&session.config.commands).map_err(error)?;
                fs::create_dir_all(&session.config.results).map_err(error)?;
                while session.active.load(Ordering::Acquire) {
                    let mut commands = fs::read_dir(&session.config.commands)
                        .map_err(error)?
                        .map(|entry| entry.map(|entry| entry.path()).map_err(error))
                        .collect::<Result<Vec<_>>>()?;
                    commands.sort();
                    for path in commands {
                        if path.extension().and_then(|extension| extension.to_str()) != Some("json")
                        {
                            continue;
                        }
                        let command: Command =
                            serde_json::from_slice(&fs::read(&path).map_err(error)?)
                                .map_err(error)?;
                        fs::remove_file(&path).map_err(error)?;
                        // DOC-12 §7.3：Kv.put 的 Vec<u8> 在 UP/1 中使用 CBOR 字节串。
                        let arguments = if self.role == "kv-client" && command.method == "put" {
                            let (key, bytes): (String, Vec<u8>) =
                                decode(&Value::Array(command.args))?;
                            vec![Value::Text(key), Value::Bytes(bytes)]
                        } else {
                            command.args
                        };
                        let started = std::time::Instant::now();
                        let result =
                            Self::invoke(&session, &command.interface, &command.method, arguments)
                                .await;
                        let rpc_wall_us = started.elapsed().as_micros() as u64;
                        let reply = match result {
                            Ok(value) => Reply {
                                ok: true,
                                code: None,
                                value,
                                detail: Value::Null,
                                rpc_wall_us,
                            },
                            Err(problem) => Reply {
                                ok: false,
                                code: Some(problem.code),
                                value: problem.message.into(),
                                detail: problem.detail,
                                rpc_wall_us,
                            },
                        };
                        let destination = session
                            .config
                            .results
                            .join(path.file_name().expect("command filename"));
                        publish(&destination, serde_json::to_vec(&reply).map_err(error)?)?;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
            _ => return Err(WireError::new(2, "fixture role")),
        }
        Ok(())
    }

    async fn invoke(
        session: &Session,
        interface: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        let found = session
            .peer
            .request(
                "get",
                value(&payload::Get {
                    context: session.id,
                    interface: interface.into(),
                })?,
                Duration::from_secs(10),
            )
            .await?;
        let metadata: payload::GetResult = decode(found.value())?;
        let object = found.import(metadata.object)?;
        let response = object
            .peer()
            .request(
                "invoke",
                value(&payload::Invoke {
                    object: object.id(),
                    method: method.into(),
                    args,
                    refs: Vec::new(),
                })?,
                Duration::from_secs(15),
            )
            .await?;
        Ok(decode::<payload::InvokeResult>(response.value())?.value)
    }
}

struct Program(Arc<Shared>);
impl Handler for Program {
    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing>> {
        let shared = self.0.clone();
        Box::pin(async move {
            let response = match method.as_str() {
                "hello" => value(&shared.description())?,
                "resolve" => {
                    let request: payload::Resolve = decode(params.value())?;
                    if decode::<Config>(&request.config)?.invalid {
                        return Err(WireError::new(1002, "fixture configuration rejected"));
                    }
                    Value::Null
                }
                "apply" => {
                    shared.apply(peer, decode(params.value())?).await?;
                    Value::Null
                }
                "callback" => {
                    shared.callback(decode(params.value())?).await?;
                    Value::Null
                }
                "invoke" => {
                    let request: payload::Invoke = decode(params.value())?;
                    let object = shared.object(request.object)?;
                    let (session, meta) = match object.as_ref() {
                        Resource::Service(session) => (session, Value::Null),
                        Resource::Scoped(session, meta) => (session, meta.clone()),
                        _ => return Err(WireError::new(2, "fixture service type")),
                    };
                    if request.method == "cordis.scope" {
                        let (meta,): (Value,) = decode(&Value::Array(request.args))?;
                        let (object, offer) = shared.offer(Resource::Scoped(session.clone(), meta));
                        return Ok(offer.outgoing(value(&payload::InvokeResult {
                            value: payload::ObjectRef(object).into(),
                            refs: vec![object],
                        })?));
                    }
                    let result = match request.method.as_str() {
                        "get" => session.counter.lock().value.into(),
                        "initial" => session.config.initial.into(),
                        "set" => {
                            let (value,): (u64,) = decode(&Value::Array(request.args))?;
                            set_counter(session, value).await?;
                            Value::Null
                        }
                        "meta" => meta,
                        _ => return Err(WireError::new(3, "fixture method")),
                    };
                    value(&payload::InvokeResult {
                        value: result,
                        refs: Vec::new(),
                    })?
                }
                "object_info" => value(&payload::ObjectInfo {
                    interface: shared.counter_interface().into(),
                    methods: vec!["get".into(), "set".into(), "initial".into(), "meta".into()],
                    local: None,
                })?,
                "export" => {
                    let request: payload::Export = decode(params.value())?;
                    let object = shared.object(request.object)?;
                    let Resource::Exporter(session) = object.as_ref() else {
                        return Err(WireError::new(2, "fixture exporter type"));
                    };
                    let mut counter = session.counter.lock();
                    counter.frozen = true;
                    value(&payload::ValueResult {
                        value: counter.value.into(),
                    })?
                }
                "thaw" => {
                    let request: payload::Thaw = decode(params.value())?;
                    let object = shared.object(request.object)?;
                    let Resource::Exporter(session) = object.as_ref() else {
                        return Err(WireError::new(2, "fixture exporter type"));
                    };
                    session.counter.lock().frozen = false;
                    Value::Null
                }
                "shutdown" if shared.mode == "shutdown-fail" => {
                    return Err(WireError::new(1003, "fixture shutdown failed"));
                }
                "shutdown" => Value::Null,
                _ => return Err(WireError::new(3, format!("fixture method {method}"))),
            };
            Ok(Outgoing::new(response))
        })
    }

    fn notify(
        &self,
        _peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<()>> {
        let shared = self.0.clone();
        Box::pin(async move {
            match method.as_str() {
                "release" => {
                    let release: payload::Release = decode(params.value())?;
                    let removed = {
                        let mut objects = shared.objects.lock();
                        release
                            .objects
                            .iter()
                            .filter_map(|id| objects.remove(id))
                            .collect::<Vec<_>>()
                    };
                    drop(removed);
                }
                "release_context" => {
                    let context: payload::Context = decode(params.value())?;
                    let removed = shared.sessions.lock().remove(&context.context);
                    drop(removed);
                }
                "callback" => shared.callback(decode(params.value())?).await?,
                _ => return Err(WireError::new(3, "fixture notification")),
            }
            Ok(())
        })
    }
}

/// DOC-11 §5.3：真实写请求在导出冻结期间排队，解冻后提交计数器值。
async fn set_counter(session: &Session, value: u64) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            {
                let mut counter = session.counter.lock();
                if !counter.frozen {
                    counter.value = value;
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .map_err(|_| WireError::new(6, "fixture write stayed frozen"))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let role = std::env::args()
        .nth(1)
        .ok_or_else(|| WireError::new(2, "fixture role required"))?;
    let mode = std::env::args().nth(2).unwrap_or_else(|| "v1".into());
    let shared = Arc::new(Shared {
        role,
        mode,
        next: AtomicU64::new(1),
        objects: Mutex::new(HashMap::new()),
        sessions: Mutex::new(HashMap::new()),
    });
    let connection = Connection::attach(
        0,
        tokio::io::stdin(),
        tokio::io::stdout(),
        ConnectionOptions::default(),
        Arc::new(Program(shared)),
    )?;
    connection.closed().await;
    Ok(())
}

/// DOC-15 验收：观察值和管理回复完整写入暂存文件后，共同通过 rename 发布。
fn publish(path: &std::path::Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    let staged = path.with_extension("next");
    fs::write(&staged, bytes).map_err(error)?;
    fs::rename(staged, path).map_err(error)
}
