#![forbid(unsafe_code)]

use cordis_up::{
    ObjectRef, Value, WireError, payload,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload, RemoteObject},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

type Result<T = ()> = std::result::Result<T, WireError>;
const CONTROL: &str = "fixture.control@1.0";
const COUNTER: &str = "fixture.counter@1.0";
const CONSUMER: &str = "fixture.consumer@1.0";
const INDEPENDENT: &str = "fixture.independent@1.0";
const EVENT: &str = "fixture.changed@1.0";

fn value<T: Serialize + ?Sized>(source: &T) -> Result<Value> {
    Value::serialized(source).map_err(|error| WireError::new(2, error.to_string()))
}
fn decode<T: DeserializeOwned>(source: &Value) -> Result<T> {
    source
        .deserialized()
        .map_err(|error| WireError::new(2, error.to_string()))
}
fn number(source: &Value) -> Result<u64> {
    source
        .as_integer()
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| WireError::new(2, "expected u64"))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    initial: u64,
    cleanup_ms: u64,
    apply_ms: u64,
    invalid: bool,
    metadata_size: usize,
    scoped: bool,
    refs: Vec<u64>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            initial: 10,
            cleanup_ms: 25,
            apply_ms: 0,
            invalid: false,
            metadata_size: 0,
            scoped: false,
            refs: Vec::new(),
        }
    }
}

struct Counter {
    version: u64,
    data: Mutex<CounterData>,
}
struct CounterData {
    value: u64,
    frozen: bool,
    retired: bool,
}
impl Counter {
    fn read(&self) -> Result<u64> {
        let data = self.data.lock();
        if data.retired {
            return Err(WireError::new(4, "counter retired"));
        }
        Ok(data.value)
    }
    fn add(&self, amount: u64) -> Result<u64> {
        let mut data = self.data.lock();
        if data.retired || data.frozen {
            return Err(WireError::new(4, "counter paused"));
        }
        data.value += amount;
        Ok(data.value)
    }
}

#[derive(Clone)]
enum Bound {
    Local(Arc<Counter>),
    Remote(RemoteObject),
}
impl Bound {
    async fn read(&self) -> Result<u64> {
        match self {
            Self::Local(counter) => counter.read(),
            Self::Remote(object) => number(&invoke(object, "get", Vec::new()).await?),
        }
    }
    async fn version(&self) -> Result<u64> {
        match self {
            Self::Local(counter) => Ok(counter.version),
            Self::Remote(object) => number(&invoke(object, "version", Vec::new()).await?),
        }
    }
}

struct Session {
    id: u64,
    role: String,
    peer: Connection,
    control: RemoteObject,
    config: Config,
    metadata_method: Option<String>,
    counter: Arc<Counter>,
    dependency: Mutex<Option<Bound>>,
    observed: Mutex<(u64, u64)>,
    events: [AtomicU64; 2],
    cycles: AtomicU64,
    alive: AtomicBool,
    retained: Mutex<HashMap<u64, RemoteObject>>,
}

enum Resource {
    Scoped {
        session: Weak<Session>,
        interface: String,
        meta: Value,
    },
    Service {
        session: Weak<Session>,
        interface: String,
    },
    Body {
        session: Arc<Session>,
        cycle: bool,
    },
    Cleanup {
        session: Arc<Session>,
        cycle: bool,
    },
    Exporter(Weak<Session>),
    Event {
        session: Weak<Session>,
        asynchronous: bool,
    },
}

struct Shared {
    mode: String,
    plugin: String,
    process: AtomicU64,
    next: AtomicU64,
    objects: Mutex<HashMap<u64, Arc<Resource>>>,
    contexts: Mutex<HashMap<u64, Arc<Session>>>,
}

struct Offer {
    shared: Weak<Shared>,
    ids: Vec<u64>,
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
        if !self.committed {
            if let Some(shared) = self.shared.upgrade() {
                shared.release(&self.ids);
            }
        }
    }
}

impl Shared {
    fn offer(self: &Arc<Self>, resource: Resource) -> (u64, Offer) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.objects.lock().insert(id, Arc::new(resource));
        (
            id,
            Offer {
                shared: Arc::downgrade(self),
                ids: vec![id],
                committed: false,
            },
        )
    }
    fn object(&self, id: u64) -> Result<Arc<Resource>> {
        self.objects
            .lock()
            .get(&id)
            .cloned()
            .ok_or_else(|| WireError::new(4, "object retired"))
    }
    fn release(&self, ids: &[u64]) {
        let removed = {
            let mut objects = self.objects.lock();
            ids.iter()
                .filter_map(|id| objects.remove(id))
                .collect::<Vec<_>>()
        };
        drop(removed);
    }
    fn methods(session: &Session) -> Vec<String> {
        let mut methods = [
            "get",
            "add",
            "version",
            "observed",
            "events",
            "emit",
            "cycles",
            "quota",
            "core",
            "release-object",
            "view-effect",
            "cordis.scope",
        ]
        .map(str::to_owned)
        .to_vec();
        if let Some(method) = &session.metadata_method {
            methods.push(method.clone());
        }
        methods
    }
    fn describe(&self) -> payload::Describe {
        let dependency = |interface: &str| payload::Dependency {
            interface: interface.into(),
            required: true,
        };
        let component =
            |name: &str, interface: &str, dependent: bool, state: bool| payload::Component {
                name: name.into(),
                inject: if dependent {
                    vec![dependency(CONTROL), dependency(COUNTER)]
                } else {
                    vec![dependency(CONTROL)]
                },
                provides: vec![interface.into()],
                listens: if dependent {
                    vec![EVENT.into()]
                } else {
                    Vec::new()
                },
                state_schema: if state {
                    "manager.counter".into()
                } else {
                    String::new()
                },
                state_version_min: 1,
                state_version_max: 1,
            };
        payload::Describe {
            protocol: 1,
            plugins: vec![payload::Plugin {
                name: self.plugin.clone(),
                version: if self.mode == "v2" || self.mode == "fail" {
                    "0.2.0".into()
                } else {
                    "0.1.0".into()
                },
                sdk_version: env!("CARGO_PKG_VERSION").into(),
                build_info: "Rust UP1 process fixture".into(),
                format: 2,
                library: None,
                components: vec![
                    component("a", CONSUMER, true, false),
                    component("b", COUNTER, false, true),
                    component("independent", INDEPENDENT, false, true),
                ],
            }],
        }
    }
    async fn get(&self, session: &Session, interface: &str) -> Result<Bound> {
        let output = session
            .peer
            .request(
                "get",
                value(&payload::Get {
                    context: session.id,
                    interface: interface.into(),
                })?,
                Duration::from_secs(5),
            )
            .await?;
        let result: payload::GetResult = decode(output.value())?;
        let remote = output.import(result.object)?;
        if let Some(local) = result.local {
            if local.process == self.process.load(Ordering::Acquire) {
                if let Resource::Service { session, .. } = self.object(local.object)?.as_ref() {
                    let local = session
                        .upgrade()
                        .ok_or_else(|| WireError::new(4, "local context retired"))?;
                    return Ok(Bound::Local(local.counter.clone()));
                }
            }
        }
        Ok(Bound::Remote(remote))
    }
    async fn control(&self, session: &Session, method: &str, args: Vec<Value>) -> Result<Value> {
        invoke(&session.control, method, args).await
    }
    async fn trace(&self, session: &Session, phase: &str, data: Value) -> Result<()> {
        self.control(session, "trace", vec![phase.into(), data])
            .await
            .map(|_| ())
    }
    async fn effect(self: &Arc<Self>, session: Arc<Session>, cycle: bool) -> Result<u64> {
        let (body, offer) = self.offer(Resource::Body {
            session: session.clone(),
            cycle,
        });
        let params = value(&payload::Effect {
            context: session.id,
            label: if cycle {
                "cycle".into()
            } else {
                "lifetime".into()
            },
            body,
            asynchronous: false,
            refs: vec![body],
        })?;
        let result = session
            .peer
            .request("effect", offer.outgoing(params), Duration::from_secs(5))
            .await?;
        Ok(decode::<payload::EffectHandle>(result.value())?.effect)
    }
    async fn release_effect(&self, session: &Session, effect: u64) -> Result<()> {
        session
            .peer
            .notify("effect_release", value(&payload::EffectHandle { effect })?)
            .await?;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match session
                    .peer
                    .request(
                        "effect_settled",
                        value(&payload::EffectHandle { effect })?,
                        Duration::from_secs(2),
                    )
                    .await
                {
                    Err(error) if error.code == 4 => return Ok(()),
                    Err(error) => return Err(error),
                    Ok(_) => tokio::task::yield_now().await,
                }
            }
        })
        .await
        .map_err(|_| WireError::new(6, "effect handle remained registered"))?
    }
    async fn dispose_effect(&self, session: &Session, effect: u64) -> Result<()> {
        session
            .peer
            .request(
                "effect_dispose",
                value(&payload::EffectHandle { effect })?,
                Duration::from_secs(5),
            )
            .await?;
        self.release_effect(session, effect).await
    }
    async fn apply(self: &Arc<Self>, peer: Connection, request: payload::Apply) -> Result<()> {
        if request.plugin != self.plugin {
            return Err(WireError::new(2, "plugin mismatch"));
        }
        let config: Config = decode(&request.config)?;
        let output = peer
            .request(
                "config",
                value(&payload::Context {
                    context: request.context,
                })?,
                Duration::from_secs(5),
            )
            .await?;
        let reported: payload::ValueResult = decode(output.value())?;
        if decode::<Config>(&reported.value)? != config {
            return Err(WireError::new(1003, "core configuration changed"));
        }
        let inherited = peer
            .request(
                "inherited_state",
                value(&payload::Context {
                    context: request.context,
                })?,
                Duration::from_secs(5),
            )
            .await?;
        let inherited: Option<payload::State> = decode(inherited.value())?;
        if value(&inherited)? != value(&request.inherited)? {
            return Err(WireError::new(1005, "apply inherited state changed"));
        }
        let initial = match request.inherited {
            Some(state) => {
                if state.schema != "manager.counter" || state.version != 1 {
                    return Err(WireError::new(2, "state schema mismatch"));
                }
                number(&state.data)?
            }
            None => config.initial,
        };
        let output = peer
            .request(
                "get",
                value(&payload::Get {
                    context: request.context,
                    interface: CONTROL.into(),
                })?,
                Duration::from_secs(5),
            )
            .await?;
        let control: payload::GetResult = decode(output.value())?;
        let metadata_method =
            (config.metadata_size > 0).then(|| format!("meta{}", "x".repeat(config.metadata_size)));
        let session = Arc::new(Session {
            id: request.context,
            role: request.component,
            peer: peer.clone(),
            control: output.import(control.object)?,
            config,
            metadata_method,
            counter: Arc::new(Counter {
                version: if self.mode == "v2" || self.mode == "fail" {
                    2
                } else {
                    1
                },
                data: Mutex::new(CounterData {
                    value: initial,
                    frozen: false,
                    retired: false,
                }),
            }),
            dependency: Mutex::new(None),
            observed: Mutex::new((initial, 0)),
            events: [AtomicU64::new(0), AtomicU64::new(0)],
            cycles: AtomicU64::new(0),
            alive: AtomicBool::new(true),
            retained: Mutex::new(HashMap::new()),
        });
        self.contexts.lock().insert(session.id, session.clone());
        self.trace(&session, "apply-start", Value::Null).await?;
        if session.config.apply_ms > 0 {
            tokio::time::sleep(Duration::from_millis(session.config.apply_ms)).await;
        }
        let lifetime = self.effect(session.clone(), false).await?;
        self.release_effect(&session, lifetime).await?;
        let interface = match session.role.as_str() {
            "a" => {
                let dependency = self.get(&session, COUNTER).await?;
                *session.observed.lock() = (dependency.read().await?, dependency.version().await?);
                *session.dependency.lock() = Some(dependency);
                for asynchronous in [false, true] {
                    let (callback, offer) = self.offer(Resource::Event {
                        session: Arc::downgrade(&session),
                        asynchronous,
                    });
                    peer.request(
                        "on",
                        offer.outgoing(value(&payload::On {
                            context: session.id,
                            event: EVENT.into(),
                            callback,
                            asynchronous,
                            refs: vec![callback],
                        })?),
                        Duration::from_secs(5),
                    )
                    .await?;
                }
                CONSUMER
            }
            "b" => COUNTER,
            "independent" => INDEPENDENT,
            _ => return Err(WireError::new(3, "unknown component")),
        };
        if session.role != "a" {
            let (exporter, offer) = self.offer(Resource::Exporter(Arc::downgrade(&session)));
            peer.request(
                "export_state",
                offer.outgoing(value(&payload::ExportState {
                    context: session.id,
                    schema: "manager.counter".into(),
                    version: 1,
                    exporter,
                    refs: vec![exporter],
                })?),
                Duration::from_secs(5),
            )
            .await?;
        }
        let (object, offer) = self.offer(Resource::Service {
            session: Arc::downgrade(&session),
            interface: interface.into(),
        });
        peer.request(
            "provide",
            offer.outgoing(value(&payload::Provide {
                context: session.id,
                interface: interface.into(),
                object,
                methods: Self::methods(&session),
                scoped: session.config.scoped,
                refs: vec![object],
            })?),
            Duration::from_secs(5),
        )
        .await?;
        if self.mode == "fail" && session.role == "b" {
            return Err(WireError::new(1001, "candidate apply failed"));
        }
        let (object, offer) = self.offer(Resource::Service {
            session: Arc::downgrade(&session),
            interface: interface.into(),
        });
        let ready = value(&payload::Invoke {
            object: session.control.id(),
            method: "ready".into(),
            args: vec![
                ObjectRef(object).into(),
                session.counter.read()?.into(),
                session.counter.version.into(),
            ],
            refs: vec![object],
        })?;
        session
            .control
            .peer()
            .request("invoke", offer.outgoing(ready), Duration::from_secs(5))
            .await?;
        if self.mode == "crash-loop" && session.role == "b" {
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                std::process::exit(70);
            });
        }
        Ok(())
    }
    async fn callback(self: &Arc<Self>, request: payload::Callback) -> Result<()> {
        let resource = self.object(request.object)?;
        match resource.as_ref() {
            Resource::Body { session, cycle } => {
                let (callback, offer) = self.offer(Resource::Cleanup {
                    session: session.clone(),
                    cycle: *cycle,
                });
                session
                    .peer
                    .request(
                        "defer",
                        offer.outgoing(value(&payload::Defer {
                            scope: request.scope,
                            callback,
                            asynchronous: true,
                            refs: vec![callback],
                        })?),
                        Duration::from_secs(5),
                    )
                    .await?;
                Ok(())
            }
            Resource::Cleanup {
                session,
                cycle: true,
            } => {
                session.cycles.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Resource::Cleanup {
                session,
                cycle: false,
            } => {
                let current = session
                    .peer
                    .request(
                        "is_current",
                        value(&payload::Context {
                            context: session.id,
                        })?,
                        Duration::from_secs(5),
                    )
                    .await?;
                if decode::<bool>(current.value())? {
                    return Err(WireError::new(1004, "cleanup retained current target"));
                }
                session
                    .peer
                    .request(
                        "config",
                        value(&payload::Context {
                            context: session.id,
                        })?,
                        Duration::from_secs(5),
                    )
                    .await?;
                if session.role == "a" {
                    self.trace(session, "a-clean-start", Value::Null).await?;
                    tokio::time::sleep(Duration::from_millis(session.config.cleanup_ms)).await;
                    let dependency = self.get(session, COUNTER).await?;
                    let value = dependency.read().await?;
                    self.trace(session, "a-clean-end", value.into()).await?;
                } else {
                    session.counter.data.lock().retired = true;
                    self.trace(
                        session,
                        if session.role == "b" {
                            "b-clean"
                        } else {
                            "independent-clean"
                        },
                        Value::Null,
                    )
                    .await?;
                }
                Ok(())
            }
            Resource::Event {
                session,
                asynchronous,
            } => {
                let Some(session) = session.upgrade() else {
                    return Ok(());
                };
                if !session.alive.load(Ordering::Acquire) {
                    return Ok(());
                }
                let amount = number(&request.payload)?;
                session.events[usize::from(*asynchronous)].fetch_add(amount, Ordering::SeqCst);
                self.trace(
                    &session,
                    if *asynchronous {
                        "event-async"
                    } else {
                        "event-sync"
                    },
                    amount.into(),
                )
                .await
            }
            _ => Err(WireError::new(2, "object is not a callback")),
        }
    }
    async fn invoke(self: &Arc<Self>, request: payload::Invoke) -> Result<Value> {
        let resource = self.object(request.object)?;
        if let Resource::Scoped { session, meta, .. } = resource.as_ref() {
            let session = session
                .upgrade()
                .ok_or_else(|| WireError::new(4, "scoped context retired"))?;
            return match request.method.as_str() {
                "meta" => Ok(meta.clone()),
                "get" => Ok(session.counter.read()?.into()),
                _ => Err(WireError::new(3, "unknown scoped method")),
            };
        }
        let Resource::Service { session, .. } = resource.as_ref() else {
            return Err(WireError::new(2, "object is not a service"));
        };
        let session = session
            .upgrade()
            .ok_or_else(|| WireError::new(4, "service context retired"))?;
        if !session.alive.load(Ordering::Acquire) {
            return Err(WireError::new(4, "service context retired"));
        }
        match request.method.as_str() {
            "core" => {
                let (method, params): (String, Value) = decode(&Value::Array(request.args))?;
                let response = session
                    .peer
                    .request(&method, params, Duration::from_secs(5))
                    .await?;
                let retained = cordis_up::payload::payload_refs(response.value())
                    .map_err(|problem| WireError::new(2, problem.to_string()))?
                    .into_iter()
                    .map(|id| response.import(id).map(|object| (id, object)))
                    .collect::<Result<Vec<_>>>()?;
                session.retained.lock().extend(retained);
                Ok(response.value().clone())
            }
            "release-object" => {
                let (id,): (u64,) = decode(&Value::Array(request.args))?;
                let removed = session.retained.lock().remove(&id);
                drop(removed);
                Ok(Value::Null)
            }
            "view-effect" => {
                let (context,): (u64,) = decode(&Value::Array(request.args))?;
                let (body, offer) = self.offer(Resource::Body {
                    session: session.clone(),
                    cycle: true,
                });
                let response = session
                    .peer
                    .request(
                        "effect",
                        offer.outgoing(value(&payload::Effect {
                            context,
                            label: "derived-effect".into(),
                            body,
                            asynchronous: false,
                            refs: vec![body],
                        })?),
                        Duration::from_secs(5),
                    )
                    .await?;
                Ok(response.value().clone())
            }
            "get" if session.role == "a" => {
                let dependency = session
                    .dependency
                    .lock()
                    .clone()
                    .ok_or_else(|| WireError::new(4, "dependency absent"))?;
                Ok(dependency.read().await?.into())
            }
            "get" => Ok(session.counter.read()?.into()),
            "add" => Ok(session
                .counter
                .add(number(
                    request
                        .args
                        .first()
                        .ok_or_else(|| WireError::new(2, "amount required"))?,
                )?)?
                .into()),
            "version" => Ok(session.counter.version.into()),
            "observed" => {
                let observed = *session.observed.lock();
                Ok(Value::Array(vec![observed.0.into(), observed.1.into()]))
            }
            "events" => Ok(Value::Array(
                session
                    .events
                    .iter()
                    .map(|count| count.load(Ordering::SeqCst).into())
                    .collect(),
            )),
            "emit" => {
                session
                    .peer
                    .request(
                        "emit",
                        value(&payload::Emit {
                            context: session.id,
                            event: EVENT.into(),
                            payload: 5u64.into(),
                            asynchronous: true,
                            refs: Vec::new(),
                        })?,
                        Duration::from_secs(5),
                    )
                    .await?;
                Ok(Value::Null)
            }
            "cycles" => {
                let before = session.cycles.load(Ordering::SeqCst);
                for _ in 0..3 {
                    let effect = self.effect(session.clone(), true).await?;
                    self.dispose_effect(&session, effect).await?;
                }
                Ok((session.cycles.load(Ordering::SeqCst) - before).into())
            }
            "quota" => {
                let mut held = Vec::new();
                let mut code = 0;
                for _ in 0..3 {
                    match self.effect(session.clone(), true).await {
                        Ok(effect) => held.push(effect),
                        Err(problem) => {
                            code = problem.code;
                            break;
                        }
                    }
                }
                let held_count = held.len();
                for effect in held {
                    self.dispose_effect(&session, effect).await?;
                }
                Ok(Value::Array(vec![
                    u64::from(code).into(),
                    (held_count as u64).into(),
                ]))
            }
            method if session.metadata_method.as_deref() == Some(method) => {
                Ok(session.counter.read()?.into())
            }
            _ => Err(WireError::new(3, "unknown method")),
        }
    }
}

async fn invoke(object: &RemoteObject, method: &str, args: Vec<Value>) -> Result<Value> {
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
            Duration::from_secs(5),
        )
        .await?;
    Ok(decode::<payload::InvokeResult>(response.value())?.value)
}

struct Program(Arc<Shared>);
impl Handler for Program {
    fn request(
        &self,
        peer: Connection,
        method: String,
        input: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing>> {
        let shared = self.0.clone();
        Box::pin(async move {
            let response = match method.as_str() {
                "hello" => {
                    let hello: payload::Hello = decode(input.value())?;
                    if hello.protocol != 1 {
                        return Err(WireError::new(3, "protocol version"));
                    }
                    shared.process.store(hello.process, Ordering::Release);
                    value(&shared.describe())?
                }
                "resolve" => {
                    let resolve: payload::Resolve = decode(input.value())?;
                    let config: Config = decode(&resolve.config)?;
                    if config.invalid {
                        return Err(WireError::new(1003, "configuration rejected"));
                    }
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
                "invoke" => {
                    let request: payload::Invoke = decode(input.value())?;
                    if request.method == "cordis.scope" {
                        let resource = shared.object(request.object)?;
                        let Resource::Service { session, interface } = resource.as_ref() else {
                            return Err(WireError::new(2, "scoped provider required"));
                        };
                        let (meta,): (Value,) = decode(&Value::Array(request.args))?;
                        let (object, offer) = shared.offer(Resource::Scoped {
                            session: session.clone(),
                            interface: interface.clone(),
                            meta,
                        });
                        return Ok(offer.outgoing(value(&payload::InvokeResult {
                            value: ObjectRef(object).into(),
                            refs: vec![object],
                        })?));
                    }
                    value(&payload::InvokeResult {
                        value: shared.invoke(request).await?,
                        refs: Vec::new(),
                    })?
                }
                "object_info" => {
                    let request: payload::ObjectInfoRequest = decode(input.value())?;
                    let resource = shared.object(request.object)?;
                    let (session, interface) = match resource.as_ref() {
                        Resource::Service { session, interface }
                        | Resource::Scoped {
                            session, interface, ..
                        } => (session, interface),
                        _ => return Err(WireError::new(2, "service metadata required")),
                    };
                    let session = session
                        .upgrade()
                        .ok_or_else(|| WireError::new(4, "context retired"))?;
                    value(&payload::ObjectInfo {
                        interface: interface.clone(),
                        methods: if matches!(resource.as_ref(), Resource::Scoped { .. }) {
                            vec!["get".into(), "meta".into()]
                        } else {
                            Shared::methods(&session)
                        },
                        local: None,
                    })?
                }
                "export" => {
                    let request: payload::Export = decode(input.value())?;
                    let resource = shared.object(request.object)?;
                    let Resource::Exporter(session) = resource.as_ref() else {
                        return Err(WireError::new(2, "exporter required"));
                    };
                    let session = session
                        .upgrade()
                        .ok_or_else(|| WireError::new(4, "context retired"))?;
                    let count = {
                        let mut data = session.counter.data.lock();
                        data.frozen = true;
                        data.value
                    };
                    value(&payload::ValueResult {
                        value: count.into(),
                    })?
                }
                "thaw" => {
                    let request: payload::Thaw = decode(input.value())?;
                    let resource = shared.object(request.object)?;
                    let Resource::Exporter(session) = resource.as_ref() else {
                        return Err(WireError::new(2, "exporter required"));
                    };
                    let session = session
                        .upgrade()
                        .ok_or_else(|| WireError::new(4, "context retired"))?;
                    session.counter.data.lock().frozen = false;
                    Value::Null
                }
                "shutdown" => Value::Null,
                _ => return Err(WireError::new(3, "unknown UP1 method")),
            };
            Ok(Outgoing::new(response))
        })
    }
    fn notify(
        &self,
        _: Connection,
        method: String,
        input: Payload,
    ) -> HandlerFuture<'_, Result<()>> {
        let shared = self.0.clone();
        Box::pin(async move {
            match method.as_str() {
                "release" => {
                    shared.release(&decode::<payload::Release>(input.value())?.objects);
                    Ok(())
                }
                "release_context" => {
                    let id = decode::<payload::Context>(input.value())?.context;
                    let removed = shared.contexts.lock().remove(&id);
                    if let Some(session) = &removed {
                        session.alive.store(false, Ordering::Release);
                    }
                    drop(removed);
                    Ok(())
                }
                "callback" => shared.callback(decode(input.value())?).await,
                _ => Err(WireError::new(3, "unknown UP1 notification")),
            }
        })
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "v1".into());
    let plugin = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "manager-fixture".into());
    let program = Arc::new(Program(Arc::new(Shared {
        mode,
        plugin,
        process: AtomicU64::new(0),
        next: AtomicU64::new(1),
        objects: Mutex::new(HashMap::new()),
        contexts: Mutex::new(HashMap::new()),
    })));
    let connection = Connection::attach(
        0,
        tokio::io::stdin(),
        tokio::io::stdout(),
        ConnectionOptions::default(),
        program,
    )?;
    let reason = connection.closed().await;
    eprintln!("fixture connection closed: {reason}");
    Ok(())
}
