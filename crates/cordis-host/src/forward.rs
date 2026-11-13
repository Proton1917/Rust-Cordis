//! DOC-16 §11、ADR-0041：授权服务经冻结 ABI 适配为原始 channel_* 消息。

use crate::{abi, host::Host, native::Owned, wire::from_wire};
use cordis_abi::{
    Buf, Error, GenericService, InvokeFuture, Object, Result, decode_arguments, encode, from_value,
    generic_object, make_invoke_future, to_value,
};
use cordis_up::{
    Value, payload,
    rpc::{Payload, RemoteObject},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

struct Receiver {
    _context: u64,
    object: Arc<Owned<Object>>,
}

#[derive(Default)]
pub(crate) struct Forwarders {
    next: AtomicU64,
    receivers: Mutex<HashMap<u64, Receiver>>,
}

/// ADR-0041：保留核心返回的授权引用，所有通道消息仍由核心按真实连接判断权限。
pub(crate) fn object(host: Host, context: u64, permission: RemoteObject) -> Result<Object> {
    generic_object(
        "cordis.forward@1.0".into(),
        vec!["open".into(), "frame".into(), "close".into()],
        Arc::new(Forward {
            host,
            context,
            _permission: permission,
        }),
        abi::environment(),
    )
}

struct Forward {
    host: Host,
    context: u64,
    _permission: RemoteObject,
}
enum Operation {
    Open(Value, Object),
    Frame(u64, Vec<u8>),
    Close(u64, String),
}

impl Forward {
    /// DOC-12 §7：异步入口返回之前克隆 Receiver 资源，原调用缓冲可随后释放。
    fn operation(&self, method: &str, args: &[u8]) -> Result<Operation> {
        let values = decode_arguments(args)?;
        match (method, values.as_slice()) {
            ("open", [metadata, Value::Tag(cordis_up::OBJECT_TAG, id)]) => {
                let number: u64 = from_value(id.as_ref().clone())?;
                let receiver = abi::environment().host().object_import(number)?;
                if receiver.interface()? != "cordis.forward-receiver@1.0" {
                    return Err(Error::new(2, "open 需要 ForwardReceiver"));
                }
                Ok(Operation::Open(metadata.clone(), receiver))
            }
            ("frame", [number, Value::Bytes(frame)]) => {
                Ok(Operation::Frame(from_value(number.clone())?, frame.clone()))
            }
            ("close", [number, Value::Text(reason)]) => Ok(Operation::Close(
                from_value(number.clone())?,
                reason.clone(),
            )),
            _ => Err(Error::new(2, "Forward 方法或参数形状错误")),
        }
    }
}
impl GenericService for Forward {
    fn invoke(&self, method: &str, args: &[u8]) -> Result<Buf> {
        self.host.0.executor.wait(self.invoke_async(method, args)?)
    }
    fn invoke_async(&self, method: &str, args: &[u8]) -> Result<InvokeFuture> {
        let operation = self.operation(method, args)?;
        let host = self.host.clone();
        let context = self.context;
        let task = self.host.0.executor.start(
            async move {
                let result = host
                    .0
                    .forward
                    .clone()
                    .execute(host.clone(), context, operation)
                    .await?;
                Buf::new(encode(&result)?, abi::environment())
            },
            false,
        );
        make_invoke_future(task, abi::environment())
    }
}

impl Forwarders {
    /// DOC-16 §11：Receiver 在 channel_open 发出前登记，首个 hello 可立即返回。
    async fn execute(
        self: Arc<Self>,
        host: Host,
        context: u64,
        operation: Operation,
    ) -> Result<Value> {
        let peer = host.connection()?;
        match operation {
            Operation::Open(metadata, receiver) => {
                let channel = self.next.fetch_add(1, Ordering::Relaxed) + 1;
                self.receivers.lock().insert(
                    channel,
                    Receiver {
                        _context: context,
                        object: Arc::new(host.0.executor.own(receiver)),
                    },
                );
                let metadata = Value::Map(vec![
                    ("context".into(), context.into()),
                    ("page".into(), metadata),
                ]);
                let result = peer
                    .request(
                        "channel_open",
                        to_value(&payload::ChannelOpen { channel, metadata })?,
                        Duration::from_secs(30),
                    )
                    .await;
                if let Err(problem) = result {
                    self.receivers.lock().remove(&channel);
                    return Err(from_wire(problem));
                }
                Ok(Value::Integer(channel.into()))
            }
            Operation::Frame(channel, frame) => {
                if !self.receivers.lock().contains_key(&channel) {
                    return Err(Error::new(4, "通道已关闭"));
                }
                peer.notify(
                    "channel_frame",
                    to_value(&payload::ChannelFrame {
                        channel,
                        frame: Value::Bytes(frame),
                    })?,
                )
                .await
                .map_err(from_wire)?;
                Ok(Value::Null)
            }
            Operation::Close(channel, reason) => {
                self.receivers.lock().remove(&channel);
                peer.notify(
                    "channel_close",
                    to_value(&payload::ChannelClose { channel, reason })?,
                )
                .await
                .map_err(from_wire)?;
                Ok(Value::Null)
            }
        }
    }

    /// DOC-15 AU-15：回程接收失败回收对应页面，保持其他页面与物理连接。
    pub async fn receive(self: Arc<Self>, host: Host, method: &str, params: Payload) -> Result<()> {
        let (channel, callback, arguments, closing) = if method == "channel_frame" {
            let request: payload::ChannelFrame = from_value(params.value().clone())?;
            (request.channel, "frame", vec![request.frame], false)
        } else {
            let request: payload::ChannelClose = from_value(params.value().clone())?;
            (
                request.channel,
                "close",
                vec![Value::Text(request.reason)],
                true,
            )
        };
        let receiver = {
            let mut receivers = self.receivers.lock();
            if closing {
                receivers.remove(&channel).map(|receiver| receiver.object)
            } else {
                receivers
                    .get(&channel)
                    .map(|receiver| receiver.object.clone())
            }
        };
        let Some(receiver) = receiver else {
            return Ok(());
        };
        let result = host
            .0
            .executor
            .run(
                async move {
                    receiver
                        .invoke_async(callback, &encode(&arguments)?)?
                        .await
                        .map(|_| ())
                },
                false,
            )
            .await;
        if let Err(problem) = result {
            self.receivers.lock().remove(&channel);
            if !closing {
                host.connection()?
                    .notify(
                        "channel_close",
                        to_value(&payload::ChannelClose {
                            channel,
                            reason: problem.to_string(),
                        })?,
                    )
                    .await
                    .map_err(from_wire)?;
            }
        }
        Ok(())
    }
}
