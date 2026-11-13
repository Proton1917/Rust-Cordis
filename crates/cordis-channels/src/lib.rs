//! DOC-16 §11、ADR-0041：转发连接授权、独立逻辑连接和有界二进制消息桥接。
#![forbid(unsafe_code)]

pub use cordis_up::payload::{ChannelClose as Close, ChannelFrame as Frame, ChannelOpen as Open};
use cordis_up::{
    Value, WireError,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use parking_lot::Mutex;
use std::{collections::HashMap, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};

type Result<T> = std::result::Result<T, WireError>;

#[derive(Clone, Debug)]
/// CP-2、CP-3：通道继承 UP/1 预算，每个转发方单独限制页面数量。
pub struct Options {
    pub connection: ConnectionOptions,
    pub max_channels: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            connection: ConnectionOptions::default(),
            max_channels: 64,
        }
    }
}

struct Entry {
    connection: Connection,
    input: mpsc::Sender<Vec<u8>>,
}
struct Forwarder {
    last: u64,
    channels: HashMap<u64, Entry>,
}

#[derive(Clone)]
/// DOC-15 §11.3：只有运行时按配置授权的物理连接可以创建页面连接。
pub struct Channels {
    options: Options,
    forwarders: Arc<Mutex<HashMap<u64, Forwarder>>>,
}

impl Channels {
    pub fn new(options: Options) -> Result<Self> {
        if options.max_channels == 0 {
            return Err(WireError::new(2, "通道数量上限需要正整数"));
        }
        Ok(Self {
            options,
            forwarders: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// ADR-0041：配置来源由运行时验证，授权按真实物理连接编号保存。
    pub fn authorize(&self, connection: u64) {
        self.forwarders
            .lock()
            .entry(connection)
            .or_insert_with(|| Forwarder {
                last: 0,
                channels: HashMap::new(),
            });
    }

    /// DOC-16 §11：登记唯一页面编号并接入完整 UP/1；握手交由现有监管器执行。
    pub fn open(
        &self,
        parent: Connection,
        request: &Open,
        id: u64,
        handler: Arc<dyn Handler>,
    ) -> Result<Connection> {
        let mut forwarders = self.forwarders.lock();
        let forwarder = forwarders
            .get_mut(&parent.id())
            .ok_or_else(|| WireError::new(9, "当前连接缺少转发授权"))?;
        if request.channel == 0 || request.channel <= forwarder.last {
            return Err(WireError::new(2, "通道编号需要单调递增"));
        }
        if forwarder.channels.len() >= self.options.max_channels {
            return Err(WireError::new(8, "转发方页面数量超过上限"));
        }
        let options = self.options.connection.clone();
        let (core, bridge) = tokio::io::duplex(options.limits.max_frame + 4);
        let (read, write) = tokio::io::split(core);
        let connection =
            Connection::attach(id, read, write, options.clone(), Arc::new(Logical(handler)))?;
        let (input, received) = mpsc::channel(options.queue_capacity);
        forwarder.last = request.channel;
        forwarder.channels.insert(
            request.channel,
            Entry {
                connection: connection.clone(),
                input,
            },
        );
        drop(forwarders);
        let channels = self.clone();
        let logical = connection.clone();
        let channel = request.channel;
        tokio::spawn(async move {
            channels
                .bridge(parent, channel, logical, bridge, received)
                .await;
        });
        Ok(connection)
    }

    /// CP-2、CP-3：浏览器帧进入该页面独有的有界队列，错误只结束目标页面。
    pub fn frame(&self, parent: u64, request: Frame) -> Result<()> {
        let forwarders = self.forwarders.lock();
        let forwarder = forwarders
            .get(&parent)
            .ok_or_else(|| WireError::new(9, "当前连接缺少转发授权"))?;
        let Some(entry) = forwarder.channels.get(&request.channel) else {
            return if request.channel <= forwarder.last {
                Ok(())
            } else {
                Err(WireError::new(4, "页面通道编号缺席"))
            };
        };
        let Value::Bytes(bytes) = request.frame else {
            entry.connection.terminate("通道帧需要 CBOR 字节串");
            return Ok(());
        };
        if bytes.len() > self.options.connection.limits.max_frame {
            entry.connection.terminate("页面帧超过长度上限");
        } else if let Err(problem) = entry.input.try_send(bytes) {
            entry
                .connection
                .terminate(&format!("页面入站队列结束或耗尽：{problem}"));
        }
        Ok(())
    }

    /// DOC-16 §11：关闭单个页面，其他页面和转发方继续运行。
    pub fn close(&self, parent: u64, request: Close) -> Result<()> {
        let mut forwarders = self.forwarders.lock();
        let forwarder = forwarders
            .get_mut(&parent)
            .ok_or_else(|| WireError::new(9, "当前连接缺少转发授权"))?;
        if let Some(entry) = forwarder.channels.remove(&request.channel) {
            entry.connection.terminate(&request.reason);
        }
        Ok(())
    }

    /// DOC-15 AU-15：转发方结束时撤销授权并结束它拥有的全部逻辑连接。
    pub fn revoke(&self, parent: u64, reason: &str) {
        if let Some(forwarder) = self.forwarders.lock().remove(&parent) {
            for entry in forwarder.channels.into_values() {
                entry.connection.terminate(reason);
            }
        }
    }

    /// DOC-16 §11：两向桥接与关闭信号共用生命周期，结束后释放页面队列。
    async fn bridge(
        &self,
        parent: Connection,
        channel: u64,
        logical: Connection,
        stream: tokio::io::DuplexStream,
        input: mpsc::Receiver<Vec<u8>>,
    ) {
        let (read, write) = tokio::io::split(stream);
        let outbound = relay_output(
            read,
            parent.clone(),
            channel,
            self.options.connection.limits.max_frame,
        );
        let inbound = relay_input(write, input);
        let reason = tokio::select! {
            reason = logical.closed() => reason,
            reason = parent.closed() => format!("转发方连接结束：{reason}"),
            result = inbound => result.err().map_or_else(|| "页面输入结束".into(), |problem| problem.to_string()),
            result = outbound => result.err().map_or_else(|| "页面输出结束".into(), |problem| problem.to_string()),
        };
        logical.terminate(&reason);
        if !parent.is_alive() {
            self.revoke(parent.id(), "转发方连接结束");
        }
        if let Some(forwarder) = self.forwarders.lock().get_mut(&parent.id()) {
            forwarder.channels.remove(&channel);
        }
        if parent.is_alive() {
            let result = async {
                let value = Value::serialized(&Close { channel, reason })
                    .map_err(|problem| WireError::new(2, problem.to_string()))?;
                parent.notify("channel_close", value).await
            }
            .await;
            if let Err(problem) = result {
                parent.terminate(&problem.to_string());
            }
        }
    }
}

/// DOC-16 §2：固定四字节长度头复用现有连接解码，转发层保留原始 CBOR 字节。
async fn relay_input(
    mut write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    mut input: mpsc::Receiver<Vec<u8>>,
) -> std::io::Result<()> {
    while let Some(frame) = input.recv().await {
        write.write_u32(frame.len() as u32).await?;
        write.write_all(&frame).await?;
    }
    write.shutdown().await
}

/// DOC-16 §11：核心输出转换为外层 channel_frame，外层预算由物理 Connection 检查。
async fn relay_output(
    mut read: tokio::io::ReadHalf<tokio::io::DuplexStream>,
    parent: Connection,
    channel: u64,
    max_frame: usize,
) -> Result<()> {
    loop {
        let length =
            read.read_u32()
                .await
                .map_err(|problem| WireError::new(4, problem.to_string()))? as usize;
        if length > max_frame {
            return Err(WireError::new(8, "页面输出帧超过长度上限"));
        }
        let mut frame = vec![0; length];
        read.read_exact(&mut frame)
            .await
            .map_err(|problem| WireError::new(4, problem.to_string()))?;
        let value = Value::serialized(&Frame {
            channel,
            frame: Value::Bytes(frame),
        })
        .map_err(|problem| WireError::new(2, problem.to_string()))?;
        parent.notify("channel_frame", value).await?;
    }
}

struct Logical(Arc<dyn Handler>);
impl Handler for Logical {
    fn immediate_notification(&self, method: &str) -> bool {
        self.0.immediate_notification(method)
    }
    /// CP-8：页面连接承载普通宿主方法，转发控制由外层连接管理。
    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing>> {
        if method.starts_with("channel_") {
            peer.terminate("页面连接出现嵌套转发请求");
            return Box::pin(async { Err(WireError::new(9, "页面连接缺少转发权限")) });
        }
        self.0.request(peer, method, params)
    }
    fn notify(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<()>> {
        if method.starts_with("channel_") {
            peer.terminate("页面连接出现嵌套转发通知");
            return Box::pin(async { Err(WireError::new(9, "页面连接缺少转发权限")) });
        }
        self.0.notify(peer, method, params)
    }
}
