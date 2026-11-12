use crate::{Limits, Message, Value, WireError};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Semaphore, mpsc, oneshot, watch},
    time::{Instant, timeout, timeout_at},
};

type Result<T> = std::result::Result<T, WireError>;
type Reply = oneshot::Sender<Result<Payload>>;
pub type HandlerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// CP-5：实现者在等待插件响应前释放自己的锁。
pub trait Handler: Send + Sync + 'static {
    /// 纯本地簿记可在读取下一帧前完成；涉及远端等待或插件执行时沿用异步队列。
    fn immediate_notification(&self, _method: &str) -> bool {
        false
    }
    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing>>;
    fn notify(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<()>>;
}

#[derive(Clone, Debug)]
pub struct ConnectionOptions {
    pub limits: Limits,
    pub max_inflight: usize,
    pub queue_capacity: usize,
    pub request_timeout: Duration,
    pub consecutive_timeouts: u32,
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            max_inflight: 64,
            queue_capacity: 64,
            request_timeout: Duration::from_secs(30),
            consecutive_timeouts: 3,
        }
    }
}

struct Inner {
    id: u64,
    options: ConnectionOptions,
    send: mpsc::Sender<Vec<u8>>,
    pending: Mutex<HashMap<u64, Reply>>,
    inbound: Mutex<HashMap<u64, Option<oneshot::Sender<()>>>>,
    next: AtomicU64,
    alive: AtomicBool,
    timeouts: Mutex<u32>,
    permits: Arc<Semaphore>,
    ended: watch::Sender<Option<String>>,
}

impl Inner {
    fn fail(&self, reason: impl Into<String>) {
        if !self.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        let reason = failure_message(reason.into());
        let pending = std::mem::take(&mut *self.pending.lock());
        let inbound = std::mem::take(&mut *self.inbound.lock());
        drop(inbound);
        for (_, reply) in pending {
            let _receiver = reply.send(Err(error(4, reason.clone())));
        }
        self.permits.close();
        self.ended.send_replace(Some(reason));
    }
}

#[derive(Clone)]
pub struct Connection(Arc<Inner>);

/// UP/1：提交回调捕获发送引用的 RAII 守卫，成功入队后转交引用所有权。
pub struct Outgoing {
    value: Value,
    commit: Option<Box<dyn FnOnce() + Send>>,
}

impl Outgoing {
    pub fn new(value: Value) -> Self {
        Self {
            value,
            commit: None,
        }
    }
    pub fn with_commit(value: Value, commit: impl FnOnce() + Send + 'static) -> Self {
        Self {
            value,
            commit: Some(Box::new(commit)),
        }
    }
    fn take_value(&mut self) -> Value {
        std::mem::replace(&mut self.value, Value::Null)
    }
    fn sent(mut self) {
        if let Some(commit) = self.commit.take() {
            commit();
        }
    }
}

impl From<Value> for Outgoing {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}

struct RemoteLease {
    peer: Connection,
    id: u64,
}

impl Drop for RemoteLease {
    fn drop(&mut self) {
        let _closed = self.peer.try_notify(
            "release",
            Value::Map(vec![(
                Value::Text("objects".into()),
                Value::Array(vec![Value::Integer(self.id.into())]),
            )]),
        );
    }
}

/// UP/1：载荷持有接收到的引用，导入的对象分别延长所需引用的寿命。
pub struct Payload {
    value: Value,
    objects: HashMap<u64, Arc<RemoteLease>>,
}

impl Payload {
    fn received(peer: &Connection, value: Value) -> Result<Self> {
        let mut objects = HashMap::new();
        for id in crate::payload_refs(&value).map_err(|problem| error(2, problem.to_string()))? {
            objects.insert(
                id,
                Arc::new(RemoteLease {
                    peer: peer.clone(),
                    id,
                }),
            );
        }
        Ok(Self { value, objects })
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn import(&self, id: u64) -> Result<RemoteObject> {
        self.objects
            .get(&id)
            .cloned()
            .map(RemoteObject)
            .ok_or_else(|| error(4, "对象编号属于该载荷引用表之外"))
    }
}

#[derive(Clone)]
pub struct RemoteObject(Arc<RemoteLease>);

impl RemoteObject {
    pub fn id(&self) -> u64 {
        self.0.id
    }
    pub fn peer(&self) -> &Connection {
        &self.0.peer
    }
}

impl Connection {
    /// CP-2–CP-5：连接拥有有界发送队列、在途请求及输入约束。
    pub fn attach<R, W>(
        id: u64,
        mut stdout: R,
        mut stdin: W,
        options: ConnectionOptions,
        handler: Arc<dyn Handler>,
    ) -> Result<Self>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        if options.max_inflight == 0
            || options.queue_capacity == 0
            || options.limits.max_frame < 64
            || options.limits.max_collection == 0
            || options.limits.max_values < 4
            || options.max_inflight > Semaphore::MAX_PERMITS
            || options.queue_capacity > Semaphore::MAX_PERMITS
            || options.consecutive_timeouts == 0
        {
            return Err(error(
                2,
                "连接需要 64 字节帧、1 个集合元素、4 个值节点及正的队列与超时阈值",
            ));
        }
        let (send, mut outbound) = mpsc::channel(options.queue_capacity);
        let (ended, _) = watch::channel(None);
        let peer = Self(Arc::new(Inner {
            id,
            permits: Arc::new(Semaphore::new(options.max_inflight)),
            options,
            send,
            pending: Mutex::new(HashMap::new()),
            inbound: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            timeouts: Mutex::new(0),
            ended,
        }));
        let weak = Arc::downgrade(&peer.0);
        let mut writer_end = peer.0.ended.subscribe();
        tokio::spawn(async move {
            loop {
                let bytes: Vec<u8> = tokio::select! {
                    _ = writer_end.changed() => break,
                    message = outbound.recv() => match message { Some(message) => message, None => break },
                };
                let write = async {
                    stdin.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
                    stdin.write_all(&bytes).await?;
                    stdin.flush().await
                };
                let result = tokio::select! {
                    _ = writer_end.changed() => break,
                    result = write => result,
                };
                if let Err(problem) = result {
                    fail(&weak, problem.to_string());
                    break;
                }
            }
        });
        let (notifications, mut notification_input) =
            mpsc::channel::<(Connection, String, Payload)>(peer.0.options.queue_capacity);
        let notification_handler = handler.clone();
        let mut notification_end = peer.0.ended.subscribe();
        tokio::spawn(async move {
            loop {
                let (peer, method, params) = tokio::select! {
                    _ = notification_end.changed() => break,
                    item = notification_input.recv() => match item { Some(item) => item, None => break },
                };
                let call = timeout(
                    peer.0.options.request_timeout,
                    notification_handler.notify(peer.clone(), method, params),
                );
                let result = tokio::select! {
                    _ = notification_end.changed() => break,
                    result = call => result,
                };
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(problem)) => {
                        peer.0.fail(problem.to_string());
                        break;
                    }
                    Err(_) => {
                        peer.0.fail("插件通知处理超时");
                        break;
                    }
                }
            }
        });
        let weak = Arc::downgrade(&peer.0);
        let input_handler = handler.clone();
        let input_permits = Arc::new(Semaphore::new(peer.0.options.max_inflight));
        let mut reader_end = peer.0.ended.subscribe();
        tokio::spawn(async move {
            loop {
                let mut length = [0; 4];
                let read = tokio::select! {
                    _ = reader_end.changed() => break,
                    read = stdout.read_exact(&mut length) => read,
                };
                if let Err(problem) = read {
                    fail(&weak, format!("插件输出结束：{problem}"));
                    break;
                }
                let Some(inner) = weak.upgrade() else { break };
                let size = u32::from_be_bytes(length) as usize;
                if size == 0 || size > inner.options.limits.max_frame {
                    inner.fail("UP/1 帧长度超出约定");
                    break;
                }
                let limits = inner.options.limits;
                drop(inner);
                let mut bytes = vec![0; size];
                let read = tokio::select! {
                    _ = reader_end.changed() => break,
                    read = stdout.read_exact(&mut bytes) => read,
                };
                if let Err(problem) = read {
                    fail(&weak, problem.to_string());
                    break;
                }
                let message = match crate::decode(&bytes, &limits) {
                    Ok(message) => message,
                    Err(problem) => {
                        fail(&weak, problem.to_string());
                        break;
                    }
                };
                let Some(inner) = weak.upgrade() else { break };
                let peer = Connection(inner);
                match message {
                    Message::Response { id, result } => {
                        let result = match result {
                            Ok(value) => match Payload::received(&peer, value) {
                                Ok(value) => Ok(value),
                                Err(problem) => {
                                    peer.0.fail(problem.to_string());
                                    break;
                                }
                            },
                            Err(problem) => Err(problem),
                        };
                        let sender = peer.0.pending.lock().remove(&id);
                        if let Some(sender) = sender {
                            *peer.0.timeouts.lock() = 0;
                            let _receiver = sender.send(result);
                        }
                    }
                    Message::Request { id, method, params } => {
                        let params = match Payload::received(&peer, params) {
                            Ok(params) => params,
                            Err(problem) => {
                                peer.0.fail(problem.to_string());
                                break;
                            }
                        };
                        let permit = match input_permits.clone().try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(_) => {
                                let result = Err(error(8, "该进程的在途请求容量已满"));
                                if peer.respond(id, result).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };
                        let (cancel, cancelled) = if method == "invoke" {
                            let (send, receive) = oneshot::channel();
                            (Some(send), Some(receive))
                        } else {
                            (None, None)
                        };
                        let duplicate = {
                            let mut inbound = peer.0.inbound.lock();
                            if let std::collections::hash_map::Entry::Vacant(entry) =
                                inbound.entry(id)
                            {
                                entry.insert(cancel);
                                false
                            } else {
                                true
                            }
                        };
                        if duplicate {
                            peer.0.fail("UP/1 在途请求编号重复");
                            break;
                        }
                        let handler = input_handler.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let call = handler.request(peer.clone(), method, params);
                            let result = match cancelled {
                                Some(cancelled) => tokio::select! {
                                    result = call => result,
                                    _ = cancelled => Err(error(7, "服务调用已取消")),
                                },
                                None => call.await,
                            };
                            let removed = peer.0.inbound.lock().remove(&id);
                            drop(removed);
                            let _closed = peer.respond(id, result).await;
                        });
                    }
                    Message::Notification { method, params } => {
                        if method == "cancel" {
                            let id = match &params {
                                Value::Map(fields) => fields.iter().find_map(|(key, value)| {
                                    (key.as_text() == Some("id"))
                                        .then(|| {
                                            value.as_integer().and_then(|id| u64::try_from(id).ok())
                                        })
                                        .flatten()
                                }),
                                _ => None,
                            };
                            let Some(id) = id else {
                                peer.0.fail("cancel 需要请求编号 id");
                                break;
                            };
                            let cancellation =
                                peer.0.inbound.lock().get_mut(&id).and_then(Option::take);
                            if let Some(cancellation) = cancellation {
                                let _finished = cancellation.send(());
                            }
                            continue;
                        }
                        let params = match Payload::received(&peer, params) {
                            Ok(params) => params,
                            Err(problem) => {
                                peer.0.fail(problem.to_string());
                                break;
                            }
                        };
                        if input_handler.immediate_notification(&method) {
                            if let Err(problem) =
                                input_handler.notify(peer.clone(), method, params).await
                            {
                                peer.0.fail(problem.to_string());
                                break;
                            }
                            continue;
                        }
                        if !matches!(
                            timeout(
                                peer.0.options.request_timeout,
                                notifications.send((peer.clone(), method, params))
                            )
                            .await,
                            Ok(Ok(()))
                        ) {
                            peer.0.fail("插件通知队列超过等待时限或已经关闭");
                            break;
                        }
                    }
                }
            }
        });
        Ok(peer)
    }

    pub fn id(&self) -> u64 {
        self.0.id
    }
    pub fn is_alive(&self) -> bool {
        self.0.alive.load(Ordering::Acquire)
    }

    pub async fn closed(&self) -> String {
        let mut receiver = self.0.ended.subscribe();
        loop {
            if let Some(reason) = receiver.borrow_and_update().clone() {
                return reason;
            }
            if receiver.changed().await.is_err() {
                return "进程监管已结束".into();
            }
        }
    }

    pub fn terminate(&self, reason: &str) {
        self.0.fail(reason);
    }

    async fn send(&self, message: Message, outgoing: Outgoing) -> Result<()> {
        let bytes = crate::encode(&message, &self.0.options.limits).map_err(encoding_error)?;
        self.send_bytes(bytes, outgoing).await
    }

    async fn send_bytes(&self, bytes: Vec<u8>, outgoing: Outgoing) -> Result<()> {
        if !self.is_alive() {
            return Err(error(4, "插件进程已结束"));
        }
        let permit = timeout(self.0.options.request_timeout, self.0.send.reserve())
            .await
            .map_err(|_| error(6, "插件发送队列等待超时"))?
            .map_err(|_| error(4, "插件管道已关闭"))?;
        outgoing.sent();
        permit.send(bytes);
        Ok(())
    }

    async fn respond(&self, id: u64, result: Result<Outgoing>) -> Result<()> {
        let (result, outgoing) = match result {
            Ok(mut outgoing) => (Ok(outgoing.take_value()), outgoing),
            Err(problem) => (Err(problem), Outgoing::new(Value::Null)),
        };
        let message = Message::Response { id, result };
        match crate::encode(&message, &self.0.options.limits) {
            Ok(bytes) => self.send_bytes(bytes, outgoing).await,
            Err(problem) => {
                drop(outgoing);
                let failure = WireError::new(encoding_error(problem).code, "");
                self.send(
                    Message::Response {
                        id,
                        result: Err(failure),
                    },
                    Outgoing::new(Value::Null),
                )
                .await
            }
        }
    }

    /// CP-3、CP-4：容量等待、排队与响应共用本次请求的截止时间。
    pub async fn request(
        &self,
        method: &str,
        params: impl Into<Outgoing>,
        duration: Duration,
    ) -> Result<Payload> {
        let mut outgoing = params.into();
        let deadline = Instant::now()
            .checked_add(duration)
            .ok_or_else(|| error(2, "请求截止时间超过时钟表示范围"))?;
        let _permit = timeout_at(deadline, self.0.permits.acquire())
            .await
            .map_err(|_| error(6, "插件请求容量等待超时"))?
            .map_err(|_| error(4, "插件进程已结束"))?;
        let id = self
            .0
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| error(8, "请求编号空间已耗尽"))?;
        let bytes = crate::encode(
            &Message::Request {
                id,
                method: method.into(),
                params: outgoing.take_value(),
            },
            &self.0.options.limits,
        )
        .map_err(encoding_error)?;
        let (send, receive) = oneshot::channel();
        {
            let mut pending = self.0.pending.lock();
            if !self.is_alive() {
                return Err(error(4, "插件进程已结束"));
            }
            pending.insert(id, send);
        }
        let mut guard = Pending {
            peer: self.clone(),
            id,
            completed: false,
        };
        let operation = async {
            let permit = self
                .0
                .send
                .reserve()
                .await
                .map_err(|_| error(4, "插件管道已关闭"))?;
            outgoing.sent();
            permit.send(bytes);
            receive.await.map_err(|_| error(4, "插件请求已结束"))?
        };
        let result = timeout_at(deadline, operation).await;
        match result {
            Ok(result) => {
                guard.completed = true;
                result
            }
            Err(_) => {
                let stop = {
                    let mut count = self.0.timeouts.lock();
                    *count += 1;
                    *count >= self.0.options.consecutive_timeouts
                };
                if stop {
                    self.0.fail("插件连续请求超时");
                }
                Err(error(6, "插件请求超过截止时间"))
            }
        }
    }

    pub async fn notify(&self, method: &str, params: impl Into<Outgoing>) -> Result<()> {
        let mut outgoing = params.into();
        self.send(
            Message::Notification {
                method: method.into(),
                params: outgoing.take_value(),
            },
            outgoing,
        )
        .await
    }

    /// CP-3、CP-6：同步资源清理在队列容量不足时终止连接，由拥有方整表回收。
    pub fn try_notify(&self, method: &str, params: impl Into<Outgoing>) -> Result<()> {
        let mut outgoing = params.into();
        if !self.is_alive() {
            return Err(error(4, "插件进程已结束"));
        }
        let bytes = crate::encode(
            &Message::Notification {
                method: method.into(),
                params: outgoing.take_value(),
            },
            &self.0.options.limits,
        )
        .map_err(encoding_error)?;
        let result = self
            .0
            .send
            .try_reserve()
            .map(|permit| {
                outgoing.sent();
                permit.send(bytes);
            })
            .map_err(|problem| error(8, problem.to_string()));
        if let Err(problem) = &result {
            self.0.fail(problem.to_string());
        }
        result
    }
}

struct Pending {
    peer: Connection,
    id: u64,
    completed: bool,
}

impl Drop for Pending {
    fn drop(&mut self) {
        let removed = self.peer.0.pending.lock().remove(&self.id);
        if !self.completed && removed.is_some() {
            let _closed = self.peer.try_notify(
                "cancel",
                Value::Map(vec![(
                    Value::Text("id".into()),
                    Value::Integer(self.id.into()),
                )]),
            );
        }
    }
}

fn fail(peer: &Weak<Inner>, reason: String) {
    if let Some(peer) = peer.upgrade() {
        peer.fail(reason);
    }
}

fn error(code: u32, message: impl Into<String>) -> WireError {
    WireError {
        code,
        message: message.into(),
        detail: Value::Null,
    }
}

fn encoding_error(problem: crate::CodecError) -> WireError {
    let code = match &problem {
        crate::CodecError::Limit(_) | crate::CodecError::Cbor(_) => 8,
        crate::CodecError::Schema(_) | crate::CodecError::TrailingBytes => 2,
    };
    WireError::new(code, problem.to_string())
}

fn failure_message(reason: String) -> String {
    const SUFFIX: &str = "（已截断）";
    if reason.len() <= 4096 {
        return reason;
    }
    let mut end = 4096 - SUFFIX.len();
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &reason[..end], SUFFIX)
}
