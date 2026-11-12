#![forbid(unsafe_code)]

use cordis_up::{
    Limits, Message, Value, WireError,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
struct State {
    value: AtomicU64,
    waiting: AtomicUsize,
    cancelled: AtomicUsize,
    next: AtomicU64,
    objects: Mutex<HashMap<u64, Arc<u64>>>,
}

struct WaitGuard(Arc<State>);
impl Drop for WaitGuard {
    fn drop(&mut self) {
        self.0.waiting.fetch_sub(1, Ordering::SeqCst);
        self.0.cancelled.fetch_add(1, Ordering::SeqCst);
    }
}

struct ExportGuard {
    state: Arc<State>,
    id: u64,
    committed: bool,
}
impl ExportGuard {
    fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for ExportGuard {
    fn drop(&mut self) {
        if !self.committed {
            self.state.objects.lock().remove(&self.id);
        }
    }
}

struct Plugin(Arc<State>);

impl Handler for Plugin {
    fn immediate_notification(&self, method: &str) -> bool {
        matches!(method, "set" | "release")
    }
    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        Box::pin(async move {
            let value = match method.as_str() {
                "hello" => Value::Map(vec![
                    (text("protocol"), number(1)),
                    (text("plugins"), Value::Array(Vec::new())),
                ]),
                "shutdown" => Value::Null,
                "stats" => Value::Array(vec![
                    number(self.0.waiting.load(Ordering::SeqCst) as u64),
                    number(self.0.cancelled.load(Ordering::SeqCst) as u64),
                    number(self.0.objects.lock().len() as u64),
                ]),
                "invoke" => {
                    let operation = field(params.value(), "method")?
                        .as_text()
                        .ok_or_else(|| WireError::new(2, "method requires text"))?;
                    match operation {
                        "add" => {
                            let args = field(params.value(), "args")?
                                .as_array()
                                .ok_or_else(|| WireError::new(2, "args requires an array"))?;
                            let mut sum = 0_u64;
                            for value in args {
                                sum = sum
                                    .checked_add(integer(value)?)
                                    .ok_or_else(|| WireError::new(2, "sum exceeds u64"))?;
                            }
                            number(sum)
                        }
                        "value" => number(self.0.value.load(Ordering::SeqCst)),
                        "nested" => {
                            let reply = peer
                                .request(
                                    "host_add",
                                    Value::Array(vec![number(7), number(11)]),
                                    Duration::from_secs(3),
                                )
                                .await?;
                            number(integer(reply.value())? * 2)
                        }
                        "wait" => {
                            self.0.waiting.fetch_add(1, Ordering::SeqCst);
                            let _guard = WaitGuard(self.0.clone());
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            Value::Null
                        }
                        "resource" => {
                            let id = self.0.next.fetch_add(1, Ordering::Relaxed) + 1;
                            self.0.objects.lock().insert(id, Arc::new(42));
                            let guard = ExportGuard {
                                state: self.0.clone(),
                                id,
                                committed: false,
                            };
                            return Ok(Outgoing::with_commit(
                                Value::Map(vec![
                                    (text("object"), number(id)),
                                    (text("refs"), Value::Array(vec![number(id)])),
                                ]),
                                move || guard.commit(),
                            ));
                        }
                        _ => return Err(WireError::new(3, "unknown method")),
                    }
                }
                _ => return Err(WireError::new(3, "unknown request")),
            };
            Ok(value.into())
        })
    }

    fn notify(
        &self,
        _: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<(), WireError>> {
        Box::pin(async move {
            match method.as_str() {
                "set" => self
                    .0
                    .value
                    .store(integer(params.value())?, Ordering::SeqCst),
                "release" => {
                    let values = field(params.value(), "objects")?
                        .as_array()
                        .ok_or_else(|| WireError::new(2, "objects requires array"))?;
                    for value in values {
                        self.0.objects.lock().remove(&integer(value)?);
                    }
                }
                _ => return Err(WireError::new(3, "unknown notification")),
            }
            Ok(())
        })
    }
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, WireError> {
    value
        .as_map()
        .and_then(|fields| {
            fields
                .iter()
                .find_map(|(name, value)| (name.as_text() == Some(key)).then_some(value))
        })
        .ok_or_else(|| WireError::new(2, format!("missing {key}")))
}
fn integer(value: &Value) -> Result<u64, WireError> {
    value
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| WireError::new(2, "expected u64"))
}
fn text(value: &str) -> Value {
    Value::Text(value.into())
}
fn number(value: u64) -> Value {
    Value::Integer(value.into())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "normal".into());
    if mode != "normal" {
        let mut output = std::io::stdout().lock();
        match mode.as_str() {
            "exit" => return,
            "descendant" => {
                let child = std::process::Command::new("/bin/sleep").arg("60").spawn().unwrap();
                eprintln!("descendant:{}", child.id());
                return;
            }
            "oversize" => output.write_all(&u32::MAX.to_be_bytes()).unwrap(),
            "malformed" => output.write_all(&[0, 0, 0, 1, 0xff]).unwrap(),
            "flood" => {
                for id in 1..=10_000 {
                    let bytes = cordis_up::encode(
                        &Message::Request {
                            id,
                            method: "invoke".into(),
                            params: Value::Null,
                        },
                        &Limits::default(),
                    )
                    .unwrap();
                    if output
                        .write_all(&(bytes.len() as u32).to_be_bytes())
                        .and_then(|()| output.write_all(&bytes))
                        .is_err()
                    {
                        return;
                    }
                }
            }
            _ => return,
        }
        let _closed = output.flush();
        drop(output);
        std::future::pending::<()>().await;
        return;
    }
    let connection = Connection::attach(
        0,
        tokio::io::stdin(),
        tokio::io::stdout(),
        ConnectionOptions::default(),
        Arc::new(Plugin(Arc::new(State::default()))),
    )
    .unwrap();
    connection.closed().await;
    std::process::exit(0);
}
