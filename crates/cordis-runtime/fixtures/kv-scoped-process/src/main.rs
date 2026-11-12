//! AU-18、DOC-15 §6.4：进程提供方通过 Scoped 视图保留同一 KV 数据并记录实际元数据。
#![forbid(unsafe_code)]

use cordis_up::{
    Limits, ObjectRef, Value, WireError, payload,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

type Result<T> = std::result::Result<T, WireError>;

#[derive(Deserialize)]
struct Config {
    meta_observed: PathBuf,
}

struct Store {
    values: Mutex<BTreeMap<String, Vec<u8>>>,
    meta_observed: PathBuf,
}

struct Shared {
    next: AtomicU64,
    objects: Mutex<HashMap<u64, Arc<Store>>>,
    stopped: Notify,
}

struct Provider(Arc<Shared>);

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
            let released = shared.objects.lock().remove(&self.id);
            drop(released);
        }
    }
}

fn error(problem: impl std::fmt::Display) -> WireError {
    WireError::new(2, problem.to_string())
}
fn encode<T: Serialize>(value: &T) -> Result<Value> {
    Value::serialized(value).map_err(error)
}
fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    value.deserialized().map_err(error)
}

fn methods() -> Vec<String> {
    ["get", "put", "keys", "flush", "cordis.scope"]
        .map(str::to_owned)
        .to_vec()
}

fn description() -> payload::Describe {
    payload::Describe {
        protocol: 1,
        plugins: vec![payload::Plugin {
            name: "kv-scoped-process".into(),
            version: "0.1.0".into(),
            sdk_version: "UP/1".into(),
            build_info: "Rust process KV provider".into(),
            format: 2,
            library: None,
            components: vec![payload::Component {
                name: "store".into(),
                inject: Vec::new(),
                provides: vec!["kv@1.0".into()],
                listens: Vec::new(),
                state_schema: String::new(),
                state_version_min: 0,
                state_version_max: 0,
            }],
        }],
    }
}

impl Shared {
    /// DOC-16 §6：每个视图取得独立编号，发送完成前由 Offer 持有释放责任。
    fn offer(self: &Arc<Self>, store: Arc<Store>) -> (u64, Offer) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.objects.lock().insert(id, store);
        (
            id,
            Offer {
                shared: Arc::downgrade(self),
                id,
                committed: false,
            },
        )
    }

    fn object(&self, id: u64) -> Result<Arc<Store>> {
        self.objects
            .lock()
            .get(&id)
            .cloned()
            .ok_or_else(|| WireError::new(4, "KV对象已释放"))
    }

    /// DOC-15 §6.4：提供方明确声明 scoped，核心按使用方的上下文建立绑定对象。
    async fn apply(self: &Arc<Self>, peer: Connection, request: payload::Apply) -> Result<()> {
        let config: Config = decode(&request.config)?;
        let (object, offer) = self.offer(Arc::new(Store {
            values: Mutex::new(BTreeMap::new()),
            meta_observed: config.meta_observed,
        }));
        peer.request(
            "provide",
            offer.outgoing(encode(&payload::Provide {
                context: request.context,
                interface: "kv@1.0".into(),
                object,
                methods: methods(),
                scoped: true,
                refs: vec![object],
            })?),
            Duration::from_secs(5),
        )
        .await?;
        Ok(())
    }

    /// AU-18：元数据原值写入 CBOR，视图与原对象共享实际键值状态。
    fn scoped(self: &Arc<Self>, store: Arc<Store>, args: &[Value]) -> Result<Outgoing> {
        let [metadata] = args else {
            return Err(error("cordis.scope 接受一个元数据值"));
        };
        let bytes = cordis_up::encode_value(metadata, &Limits::DEFAULT).map_err(error)?;
        fs::write(&store.meta_observed, bytes).map_err(error)?;
        let (object, offer) = self.offer(store);
        Ok(offer.outgoing(encode(&payload::InvokeResult {
            value: ObjectRef(object).into(),
            refs: vec![object],
        })?))
    }

    fn invoke(self: &Arc<Self>, request: payload::Invoke) -> Result<Outgoing> {
        let store = self.object(request.object)?;
        if request.method == "cordis.scope" {
            return self.scoped(store, &request.args);
        }
        let value = match (request.method.as_str(), request.args.as_slice()) {
            ("get", [Value::Text(key)]) => store
                .values
                .lock()
                .get(key)
                .cloned()
                .map_or(Value::Null, Value::Bytes),
            ("put", [Value::Text(key), Value::Bytes(bytes)]) => {
                store.values.lock().insert(key.clone(), bytes.clone());
                Value::Null
            }
            ("keys", []) => Value::Array(
                store
                    .values
                    .lock()
                    .keys()
                    .cloned()
                    .map(Value::Text)
                    .collect(),
            ),
            ("flush", []) => Value::Null,
            ("get" | "put" | "keys" | "flush", _) => return Err(error("KV参数类型与接口契约不符")),
            _ => return Err(WireError::new(3, "KV方法缺席")),
        };
        Ok(Outgoing::new(encode(&payload::InvokeResult {
            value,
            refs: Vec::new(),
        })?))
    }
}

impl Handler for Provider {
    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing>> {
        Box::pin(async move {
            let shared = &self.0;
            let value = match method.as_str() {
                "hello" => {
                    let hello: payload::Hello = decode(params.value())?;
                    if hello.protocol != 1 {
                        return Err(WireError::new(3, "需要UP/1协议"));
                    }
                    encode(&description())?
                }
                "resolve" => {
                    let request: payload::Resolve = decode(params.value())?;
                    let _: Config = decode(&request.config)?;
                    Value::Null
                }
                "apply" => {
                    shared.apply(peer, decode(params.value())?).await?;
                    Value::Null
                }
                "invoke" => return shared.invoke(decode(params.value())?),
                "object_info" => {
                    let request: payload::ObjectInfoRequest = decode(params.value())?;
                    shared.object(request.object)?;
                    encode(&payload::ObjectInfo {
                        interface: "kv@1.0".into(),
                        methods: methods(),
                        local: None,
                    })?
                }
                "shutdown" => {
                    let shared = shared.clone();
                    return Ok(
                        Outgoing::new(Value::Null).after_write(move || shared.stopped.notify_one())
                    );
                }
                _ => return Err(WireError::new(3, "进程KV请求方法缺席")),
            };
            Ok(Outgoing::new(value))
        })
    }

    fn notify(
        &self,
        _peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<()>> {
        Box::pin(async move {
            match method.as_str() {
                "release" => {
                    let released: payload::Release = decode(params.value())?;
                    let mut objects = self.0.objects.lock();
                    for id in released.objects {
                        objects.remove(&id);
                    }
                }
                "release_context" => {}
                _ => return Err(WireError::new(3, "进程KV通知方法缺席")),
            }
            Ok(())
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let shared = Arc::new(Shared {
        next: AtomicU64::new(1),
        objects: Mutex::new(HashMap::new()),
        stopped: Notify::new(),
    });
    let peer = Connection::attach(
        0,
        tokio::io::stdin(),
        tokio::io::stdout(),
        ConnectionOptions::default(),
        Arc::new(Provider(shared.clone())),
    )?;
    tokio::select! { _ = peer.closed() => {}, _ = shared.stopped.notified() => {} }
    Ok(())
}
