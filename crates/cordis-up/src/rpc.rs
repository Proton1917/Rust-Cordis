mod buffer;

use crate::{Limits, Message, Value, WireError};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    future::{Future, poll_fn},
    pin::Pin,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Poll, Waker},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
    time::{Instant, timeout, timeout_at},
};

type Result<T> = std::result::Result<T, WireError>;
type Reply = oneshot::Sender<Result<Payload>>;
type Ended = watch::Receiver<Option<String>>;
type Notification = (Connection, String, Payload);
pub type HandlerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

struct PendingReply {
    sender: Reply,
    owns_deadline: bool,
    write: Arc<RequestWrite>,
}

struct RequestWrite {
    state: Mutex<RequestWriteState>,
}

struct RequestWriteState {
    started: bool,
    cancelled: bool,
    commit: Option<Box<dyn FnOnce() + Send>>,
    waiter: Option<Waker>,
}

impl RequestWrite {
    // The same lock covers the first successful poll_write and cancellation.
    // Only a frame that has transferred bytes needs a following cancel message.
    fn cancel(&self) -> bool {
        let (started, commit, waiter) = {
            let mut state = self.state.lock();
            state.cancelled = true;
            let commit = if state.started {
                None
            } else {
                state.commit.take()
            };
            (state.started, commit, state.waiter.take())
        };
        drop(commit);
        if let Some(waiter) = waiter {
            waiter.wake();
        }
        started
    }
}

const INVOKE_BUDGET_CAPABILITY: &str = "invoke_budget_v1";
const INVOKE_BUDGET_FIELD: &str = "budget_ms";

tokio::task_local! {
    static INVOCATION_DEADLINE: Instant;
}

fn invocation_deadline() -> Option<Instant> {
    INVOCATION_DEADLINE.try_with(|deadline| *deadline).ok()
}

/// 将当前调用的截止时间交给执行这个调用的另一个任务。
pub fn inherit_invocation<F: Future>(call: F) -> impl Future<Output = F::Output> {
    let deadline = invocation_deadline();
    async move {
        match deadline {
            Some(deadline) => INVOCATION_DEADLINE.scope(deadline, call).await,
            None => call.await,
        }
    }
}

/// 按本地预算和父调用截止时间中较早的时刻返回结果。
pub async fn bound_invocation<T>(
    duration: Duration,
    call: impl Future<Output = Result<T>>,
) -> Result<T> {
    let local = deadline_after(duration)?;
    let deadline = invocation_deadline().map_or(local, |parent| local.min(parent));
    at_invocation_deadline(deadline, call).await
}

async fn at_invocation_deadline<T>(
    deadline: Instant,
    call: impl Future<Output = Result<T>>,
) -> Result<T> {
    if Instant::now() >= deadline {
        return Err(error(6, "服务调用超过截止时间"));
    }
    INVOCATION_DEADLINE
        .scope(deadline, timeout_at(deadline, call))
        .await
        .map_err(|_| error(6, "服务调用超过截止时间"))?
}

fn deadline_after(duration: Duration) -> Result<Instant> {
    Instant::now()
        .checked_add(duration)
        .ok_or_else(|| error(2, "请求截止时间超过时钟表示范围"))
}

fn parameter_field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    let Value::Map(fields) = value else {
        return None;
    };
    fields
        .iter()
        .find_map(|(key, value)| (key.as_text() == Some(name)).then_some(value))
}

fn set_parameter(value: &mut Value, name: &str, parameter: Value) {
    if let Value::Map(fields) = value {
        match fields
            .iter_mut()
            .find(|(key, _)| key.as_text() == Some(name))
        {
            Some((_, value)) => *value = parameter,
            None => fields.push((Value::Text(name.into()), parameter)),
        }
    }
}

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
    /// Capacity held from encoding through queued and partial writes.
    pub max_buffered_bytes: usize,
    /// Additional byte capacity available to responses and resource controls.
    pub control_reserve_bytes: usize,
    /// Reserved FIFO admission slots for responses, cancel and release.
    pub control_queue_capacity: usize,
    pub request_timeout: Duration,
    pub consecutive_timeouts: u32,
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            max_inflight: 64,
            queue_capacity: 64,
            max_buffered_bytes: 64 * 1024 * 1024,
            control_reserve_bytes: 64 * 1024,
            control_queue_capacity: 64,
            request_timeout: Duration::from_secs(30),
            consecutive_timeouts: 3,
        }
    }
}

impl ConnectionOptions {
    /// DOC-15 CP-2、CP-3：连接预算覆盖控制消息及有界并发。
    fn validate(&self) -> Result<()> {
        if self.max_inflight == 0
            || self.queue_capacity == 0
            || self.control_queue_capacity == 0
            || self.max_buffered_bytes < 64
            || self.control_reserve_bytes < 64
            || self
                .max_buffered_bytes
                .checked_add(self.control_reserve_bytes)
                .is_none()
            || self.limits.max_frame.checked_add(4).is_none()
            || self
                .queue_capacity
                .checked_add(self.control_queue_capacity)
                .is_none_or(|capacity| capacity > Semaphore::MAX_PERMITS)
            || self.limits.max_frame < 64
            || self.limits.max_collection == 0
            || self.limits.max_values < 4
            || self.max_inflight > Semaphore::MAX_PERMITS
            || self.queue_capacity > Semaphore::MAX_PERMITS
            || self.consecutive_timeouts == 0
        {
            return Err(error(
                2,
                "连接需要 64 字节帧、1 个集合元素、4 个值节点及正的队列与超时阈值",
            ));
        }
        Ok(())
    }
}

struct Inner {
    id: u64,
    options: ConnectionOptions,
    send: mpsc::Sender<Outbound>,
    data_slots: Arc<Semaphore>,
    control_slots: Arc<Semaphore>,
    bytes: Arc<buffer::Budget>,
    pending: Mutex<HashMap<u64, PendingReply>>,
    inbound: Mutex<HashMap<u64, Option<oneshot::Sender<()>>>>,
    next: AtomicU64,
    alive: AtomicBool,
    peer_budget: AtomicBool,
    timeouts: Mutex<u32>,
    permits: Arc<Semaphore>,
    ended: watch::Sender<Option<String>>,
}

impl Inner {
    // Claim and account for completion under the same pending lock. None means
    // cancellation without a health outcome; callers terminate only after return.
    fn claim_pending(&self, id: u64, timed_out: Option<bool>) -> (Option<PendingReply>, bool) {
        let mut pending = self.pending.lock();
        let Some(reply) = pending.remove(&id) else {
            return (None, false);
        };
        let stop = match timed_out {
            Some(timed_out) if !timed_out || reply.owns_deadline => {
                let mut count = self.timeouts.lock();
                if timed_out {
                    *count = count.saturating_add(1);
                    *count >= self.options.consecutive_timeouts
                } else {
                    *count = 0;
                    false
                }
            }
            _ => false,
        };
        (Some(reply), stop)
    }

    fn fail(&self, reason: impl Into<String>) {
        if !self.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        let reason = failure_message(reason.into());
        let pending = std::mem::take(&mut *self.pending.lock());
        let inbound = std::mem::take(&mut *self.inbound.lock());
        drop(inbound);
        for (_, reply) in pending {
            reply.write.cancel();
            let _receiver = reply.sender.send(Err(error(4, reason.clone())));
        }
        self.permits.close();
        self.data_slots.close();
        self.control_slots.close();
        self.ended.send_replace(Some(reason));
    }
}

#[derive(Clone)]
pub struct Connection(Arc<Inner>);

/// UP/1：提交回调捕获出口引用的 RAII 守卫；可撤回请求在首字节写出时转交引用。
pub struct Outgoing {
    value: Value,
    commit: Option<Box<dyn FnOnce() + Send>>,
    after_write: Option<Box<dyn FnOnce() + Send>>,
}

struct Outbound {
    // 帧头与消息体共用缓冲，写端按完整帧推进部分写入。
    bytes: Vec<u8>,
    after_write: Option<Box<dyn FnOnce() + Send>>,
    request: Option<QueuedRequest>,
    _bytes: buffer::Lease,
    queue_slot: Option<OwnedSemaphorePermit>,
}

struct QueuedRequest {
    id: u64,
    deadline: Instant,
    budget_offset: Option<usize>,
    write: Arc<RequestWrite>,
}

impl Outgoing {
    pub fn new(value: Value) -> Self {
        Self {
            value,
            commit: None,
            after_write: None,
        }
    }
    pub fn with_commit(value: Value, commit: impl FnOnce() + Send + 'static) -> Self {
        Self {
            value,
            commit: Some(Box::new(commit)),
            after_write: None,
        }
    }
    pub fn after_write(mut self, action: impl FnOnce() + Send + 'static) -> Self {
        self.after_write = Some(Box::new(action));
        self
    }
    fn take_value(&mut self) -> Value {
        std::mem::replace(&mut self.value, Value::Null)
    }
    fn sent(mut self, encoded: buffer::Encoded, slot: OwnedSemaphorePermit) -> Outbound {
        if let Some(commit) = self.commit.take() {
            commit();
        }
        Outbound {
            bytes: encoded.bytes,
            after_write: self.after_write.take(),
            request: None,
            _bytes: encoded.lease,
            queue_slot: Some(slot),
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
    wire_bytes: usize,
    has_resource_tags: bool,
    objects: HashMap<u64, Arc<RemoteLease>>,
}

impl Payload {
    fn received(
        peer: &Connection,
        value: Value,
        wire_bytes: usize,
        has_resource_tags: bool,
    ) -> Result<Self> {
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
        Ok(Self {
            value,
            wire_bytes,
            has_resource_tags,
            objects,
        })
    }

    /// Original CBOR message body bytes, measured once at the connection reader.
    pub fn wire_bytes(&self) -> usize {
        self.wire_bytes
    }

    /// 解码该载荷时是否出现过 `OBJECT_TAG`（60000）标签；无标签时调用方可跳过值树资源扫描。
    pub fn has_resource_tags(&self) -> bool {
        self.has_resource_tags
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    /// 转交业务值，引用租约仍由本载荷和已导入的对象持有。
    /// 调用方在完成资源导入前保留本载荷；再次取值返回 `Value::Null`。
    pub fn take_value(&mut self) -> Value {
        std::mem::replace(&mut self.value, Value::Null)
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
    /// DOC-15 CP-2–CP-5、ADR-0018：连接分别调度读帧、写帧和有序通知。
    pub fn attach<R, W>(
        id: u64,
        stdout: R,
        stdin: W,
        options: ConnectionOptions,
        handler: Arc<dyn Handler>,
    ) -> Result<Self>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        options.validate()?;
        let (send, outbound) =
            mpsc::channel(options.queue_capacity + options.control_queue_capacity);
        let (ended, _) = watch::channel(None);
        let peer = Self(Arc::new(Inner {
            id,
            permits: Arc::new(Semaphore::new(options.max_inflight)),
            data_slots: Arc::new(Semaphore::new(options.queue_capacity)),
            control_slots: Arc::new(Semaphore::new(options.control_queue_capacity)),
            bytes: buffer::Budget::new(options.max_buffered_bytes, options.control_reserve_bytes),
            options,
            send,
            pending: Mutex::new(HashMap::new()),
            inbound: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            peer_budget: AtomicBool::new(false),
            timeouts: Mutex::new(0),
            ended,
        }));
        let (notifications, notification_input) = mpsc::channel(peer.0.options.queue_capacity);
        tokio::spawn(write_frames(
            stdin,
            Arc::downgrade(&peer.0),
            outbound,
            peer.0.ended.subscribe(),
        ));
        tokio::spawn(serve_notifications(
            handler.clone(),
            notification_input,
            peer.0.ended.subscribe(),
        ));
        tokio::spawn(read_frames(
            stdout,
            Arc::downgrade(&peer.0),
            handler,
            Arc::new(Semaphore::new(peer.0.options.max_inflight)),
            notifications,
            peer.0.ended.subscribe(),
        ));
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

    /// Current encoded-buffer capacity, including queue waits and partial writes.
    pub fn buffered_bytes(&self) -> usize {
        self.0.bytes.bytes()
    }

    /// Peak encoded-buffer capacity since connection creation.
    pub fn peak_buffered_bytes(&self) -> usize {
        self.0.bytes.peak()
    }

    fn encode(
        &self,
        message: &Message,
        retire_on_quota: bool,
        response_label: Option<&str>,
    ) -> Result<buffer::Encoded> {
        let control = matches!(message, Message::Response { .. })
            || matches!(message, Message::Notification { method, .. } if matches!(method.as_str(), "cancel" | "release"));
        self.0
            .bytes
            .encode(message, &self.0.options.limits, control)
            .map_err(|failure| {
                let problem = if failure.exhausted {
                    error(8, "连接发送缓冲字节额度耗尽")
                } else {
                    encoding_error(
                        failure.problem,
                        &response_label
                            .map_or_else(|| describe(message), |label| format!("{label} 的返回值")),
                    )
                };
                if failure.exhausted && retire_on_quota {
                    self.0.fail(problem.to_string());
                }
                problem
            })
    }

    async fn send(&self, message: Message, outgoing: Outgoing) -> Result<()> {
        let bytes = self.encode(&message, false, None)?;
        // 等待发送容量时只保留编码结果，业务值的生命周期止于编码完成。
        drop(message);
        self.send_bytes(bytes, outgoing).await
    }

    async fn send_bytes(&self, bytes: buffer::Encoded, outgoing: Outgoing) -> Result<()> {
        if !self.is_alive() {
            return Err(error(4, "插件进程已结束"));
        }
        let deadline = deadline_after(self.0.options.request_timeout)?;
        let slots = if bytes.control {
            &self.0.control_slots
        } else {
            &self.0.data_slots
        };
        let slot = timeout_at(deadline, slots.clone().acquire_owned())
            .await
            .map_err(|_| error(6, "插件发送队列等待超时"))?
            .map_err(|_| error(4, "插件管道已关闭"))?;
        let permit = timeout_at(deadline, self.0.send.reserve())
            .await
            .map_err(|_| error(6, "插件发送队列等待超时"))?
            .map_err(|_| error(4, "插件管道已关闭"))?;
        permit.send(outgoing.sent(bytes, slot));
        Ok(())
    }

    /// CP-8：返回值超限时，调用方收到的错误说明调用目标与超出的容量项。
    async fn respond(&self, id: u64, label: &str, result: Result<Outgoing>) -> Result<()> {
        let (result, outgoing) = match result {
            Ok(mut outgoing) => (Ok(outgoing.take_value()), outgoing),
            Err(problem) => (Err(problem), Outgoing::new(Value::Null)),
        };
        let message = Message::Response { id, result };
        let encoded = self.encode(&message, false, Some(label));
        drop(message);
        match encoded {
            Ok(bytes) => self.send_bytes(bytes, outgoing).await,
            Err(problem) => {
                drop(outgoing);
                let failure = problem;
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
        let own_deadline = deadline_after(duration)?;
        let deadline =
            invocation_deadline().map_or(own_deadline, |parent| own_deadline.min(parent));
        let _permit = timeout_at(deadline, self.0.permits.acquire())
            .await
            .map_err(|_| error(6, "插件请求容量等待超时"))?
            .map_err(|_| error(4, "插件进程已结束"))?;
        let id = self
            .0
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| error(8, "请求编号空间已耗尽"))?;
        let mut params = outgoing.take_value();
        let operation = async {
            let slot = self
                .0
                .data_slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| error(4, "插件管道已关闭"))?;
            let permit = self
                .0
                .send
                .reserve()
                .await
                .map_err(|_| error(4, "插件管道已关闭"))?;
            let budget = if method == "invoke"
                && self.0.peer_budget.load(Ordering::Acquire)
                && let Value::Map(fields) = &mut params
            {
                // Keep the negotiated budget last so the writer can replace its
                // CBOR integer in place without retaining or cloning the value tree.
                let remaining = remaining_budget(deadline);
                fields.retain(|(key, _)| key.as_text() != Some(INVOKE_BUDGET_FIELD));
                fields.push((INVOKE_BUDGET_FIELD.into(), Value::from(remaining)));
                Some(remaining)
            } else {
                None
            };
            let bytes = self.encode(
                &Message::Request {
                    id,
                    method: method.into(),
                    params,
                },
                false,
                None,
            )?;
            if Instant::now() >= deadline {
                return Err(error(6, "插件请求在入队前超过截止时间"));
            }
            let budget_offset =
                budget.map(|remaining| bytes.bytes.len() - integer_width(remaining));
            let write = Arc::new(RequestWrite {
                state: Mutex::new(RequestWriteState {
                    started: false,
                    cancelled: false,
                    commit: outgoing.commit.take(),
                    waiter: None,
                }),
            });
            let (send, receive) = oneshot::channel();
            {
                let mut pending = self.0.pending.lock();
                if !self.is_alive() {
                    return Err(error(4, "插件进程已结束"));
                }
                pending.insert(
                    id,
                    PendingReply {
                        sender: send,
                        owns_deadline: deadline == own_deadline,
                        write: write.clone(),
                    },
                );
            }
            let _pending = Pending { peer: self, id };
            permit.send(Outbound {
                bytes: bytes.bytes,
                _bytes: bytes.lease,
                queue_slot: Some(slot),
                after_write: outgoing.after_write.take(),
                request: Some(QueuedRequest {
                    id,
                    deadline,
                    budget_offset,
                    write,
                }),
            });
            // 本地排队或编码错误不作为远端响应重置连续超时计数。
            receive.await.map_err(|_| error(4, "插件请求已结束"))
        };
        // Keep the operation alive until timeout processing claims its pending
        // entry. Response reception and local expiry then count each request once.
        let mut operation = std::pin::pin!(operation);
        let result = timeout_at(deadline, &mut operation).await;
        match result {
            Ok(Ok(result)) => {
                if method == "hello"
                    && result.as_ref().is_ok_and(|reply| {
                        parameter_field(reply.value(), INVOKE_BUDGET_CAPABILITY)
                            == Some(&Value::Bool(true))
                    })
                {
                    self.0.peer_budget.store(true, Ordering::Release);
                }
                result
            }
            Ok(Err(problem)) => Err(problem),
            Err(_) => {
                self.cancel_pending(id, true);
                Err(error(6, "插件请求超过截止时间"))
            }
        }
    }

    fn cancel_pending(&self, id: u64, timed_out: bool) {
        let (removed, stop) = self.0.claim_pending(id, timed_out.then_some(true));
        if stop {
            self.0.fail("插件连续请求超时");
        }
        let Some(removed) = removed else { return };
        let started = removed.write.cancel();
        if timed_out {
            let _receiver = removed.sender.send(Err(error(6, "插件请求超过截止时间")));
        }
        if started {
            let _closed = self.try_notify(
                "cancel",
                Value::Map(vec![(Value::Text("id".into()), Value::Integer(id.into()))]),
            );
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
        let bytes = self.encode(
            &Message::Notification {
                method: method.into(),
                params: outgoing.take_value(),
            },
            true,
            None,
        )?;
        let slots = if bytes.control {
            &self.0.control_slots
        } else {
            &self.0.data_slots
        };
        let result = slots
            .clone()
            .try_acquire_owned()
            .map_err(|problem| error(8, problem.to_string()))
            .and_then(|slot| {
                self.0
                    .send
                    .try_reserve()
                    .map(|permit| {
                        permit.send(outgoing.sent(bytes, slot));
                    })
                    .map_err(|problem| error(8, problem.to_string()))
            });
        if let Err(problem) = &result {
            self.0.fail(problem.to_string());
        }
        result
    }
}

fn remaining_budget(deadline: Instant) -> u64 {
    u64::try_from(
        deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1),
    )
    .unwrap_or(u64::MAX)
}

fn integer_width(value: u64) -> usize {
    match value {
        0..=23 => 1,
        24..=255 => 2,
        256..=65535 => 3,
        65536..=4294967295 => 5,
        _ => 9,
    }
}

fn update_budget(bytes: &mut Vec<u8>, offset: usize, value: u64) {
    let width = integer_width(value);
    bytes.truncate(offset);
    match width {
        1 => bytes.push(value as u8),
        2 => {
            bytes.push(24);
            bytes.push(value as u8);
        }
        3 => {
            bytes.push(25);
            bytes.extend_from_slice(&(value as u16).to_be_bytes());
        }
        5 => {
            bytes.push(26);
            bytes.extend_from_slice(&(value as u32).to_be_bytes());
        }
        _ => {
            bytes.push(27);
            bytes.extend_from_slice(&value.to_be_bytes());
        }
    }
    let length = (bytes.len() - 4) as u32;
    bytes[..4].copy_from_slice(&length.to_be_bytes());
}

/// DOC-16 §2、ADR-0024：帧头、消息体与flush完成后执行管理响应钩子。
async fn write_frames<W: AsyncWrite + Unpin>(
    mut output: W,
    peer: Weak<Inner>,
    mut outbound: mpsc::Receiver<Outbound>,
    mut ended: Ended,
) {
    loop {
        let mut frame = tokio::select! {
            _ = ended.changed() => break,
            frame = outbound.recv() => match frame { Some(frame) => frame, None => break },
        };
        drop(frame.queue_slot.take());
        let write = async {
            let start = if let Some(request) = &frame.request {
                let mut expired = false;
                let first = poll_fn(|cx| {
                    let mut state = request.write.state.lock();
                    if state.cancelled {
                        return Poll::Ready(Ok(None));
                    }
                    if Instant::now() >= request.deadline {
                        expired = true;
                        return Poll::Ready(Ok(None));
                    }
                    if let Some(offset) = request.budget_offset {
                        update_budget(&mut frame.bytes, offset, remaining_budget(request.deadline));
                    }
                    match Pin::new(&mut output).poll_write(cx, &frame.bytes) {
                        Poll::Pending => {
                            state.waiter = Some(cx.waker().clone());
                            Poll::Pending
                        }
                        Poll::Ready(Ok(0)) => Poll::Ready(Err(std::io::Error::new(
                            std::io::ErrorKind::WriteZero,
                            "UP/1 frame write returned zero",
                        ))),
                        Poll::Ready(Ok(count)) => {
                            state.started = true;
                            state.waiter = None;
                            let commit = state.commit.take();
                            drop(state);
                            if let Some(commit) = commit {
                                commit();
                            }
                            Poll::Ready(Ok(Some(count)))
                        }
                        Poll::Ready(Err(problem)) => Poll::Ready(Err(problem)),
                    }
                });
                match timeout_at(request.deadline, first).await {
                    Ok(Ok(Some(count))) => count,
                    Ok(Ok(None)) => {
                        if expired && let Some(inner) = peer.upgrade() {
                            Connection(inner).cancel_pending(request.id, true);
                        }
                        return Ok(false);
                    }
                    Ok(Err(problem)) => return Err(problem),
                    Err(_) => {
                        if let Some(inner) = peer.upgrade() {
                            Connection(inner).cancel_pending(request.id, true);
                        }
                        return Ok(false);
                    }
                }
            } else {
                0
            };
            output.write_all(&frame.bytes[start..]).await?;
            output.flush().await?;
            Ok(true)
        };
        let result = tokio::select! {
            _ = ended.changed() => break,
            result = write => result,
        };
        match result {
            Err(problem) => {
                fail(&peer, problem.to_string());
                break;
            }
            Ok(false) => continue,
            Ok(true) => {}
        }
        drop(frame.bytes);
        drop(frame._bytes);
        if let Some(action) = frame.after_write.take() {
            action();
        }
    }
}

/// DOC-15 CP-3–CP-5、DOC-16 §7–§8：FIFO维持callback与release_context顺序。
async fn serve_notifications(
    handler: Arc<dyn Handler>,
    mut notifications: mpsc::Receiver<Notification>,
    mut ended: Ended,
) {
    loop {
        let (peer, method, params) = tokio::select! {
            _ = ended.changed() => break,
            item = notifications.recv() => match item { Some(item) => item, None => break },
        };
        let call = timeout(
            peer.0.options.request_timeout,
            handler.notify(peer.clone(), method, params),
        );
        let result = tokio::select! {
            _ = ended.changed() => break,
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
}

#[derive(Clone, Copy)]
struct FrameInfo {
    wire_bytes: usize,
    has_resource_tags: bool,
}

/// 读帧复用的常规缓冲上限，超过该尺寸的大帧解码后归还内存以控制稳态驻留。
const RETAINED_READ_CAPACITY: usize = 64 * 1024;

/// DOC-16 §2–§4：读取任务持续分派消息，通知中的远端调用由独立读取路径接收响应。
async fn read_frames<R: AsyncRead + Unpin>(
    mut input: R,
    peer: Weak<Inner>,
    handler: Arc<dyn Handler>,
    permits: Arc<Semaphore>,
    notifications: mpsc::Sender<Notification>,
    mut ended: Ended,
) {
    let mut buffer = Vec::new();
    while let Some((message, frame)) = read_frame(&mut input, &peer, &mut ended, &mut buffer).await
    {
        let Some(inner) = peer.upgrade() else { break };
        if !dispatch_message(
            Connection(inner),
            message,
            frame,
            &handler,
            &permits,
            &notifications,
        )
        .await
        {
            break;
        }
    }
}

/// DOC-15 CP-2、CP-8：长度约束通过后复用或分配消息体缓冲，完整输入交给受限CBOR解码器。
async fn read_frame<R: AsyncRead + Unpin>(
    input: &mut R,
    peer: &Weak<Inner>,
    ended: &mut Ended,
    buffer: &mut Vec<u8>,
) -> Option<(Message, FrameInfo)> {
    let mut length = [0; 4];
    let read = tokio::select! {
        _ = ended.changed() => return None,
        read = input.read_exact(&mut length) => read,
    };
    if let Err(problem) = read {
        fail(peer, format!("插件输出结束：{problem}"));
        return None;
    }
    let inner = peer.upgrade()?;
    let size = u32::from_be_bytes(length) as usize;
    if size == 0 || size > inner.options.limits.max_frame {
        inner.fail("UP/1 帧长度超出约定");
        return None;
    }
    let limits = inner.options.limits;
    drop(inner);
    buffer.resize(size, 0);
    let read = tokio::select! {
        _ = ended.changed() => return None,
        read = input.read_exact(buffer) => read,
    };
    if let Err(problem) = read {
        fail(peer, problem.to_string());
        return None;
    }
    let decoded = crate::codec::decode_with_tags(buffer, &limits);
    if buffer.capacity() > RETAINED_READ_CAPACITY {
        *buffer = Vec::new();
    }
    match decoded {
        Ok((message, has_resource_tags)) => Some((
            message,
            FrameInfo {
                wire_bytes: size,
                has_resource_tags,
            },
        )),
        Err(problem) => {
            fail(peer, problem.to_string());
            None
        }
    }
}

async fn dispatch_message(
    peer: Connection,
    message: Message,
    frame: FrameInfo,
    handler: &Arc<dyn Handler>,
    permits: &Arc<Semaphore>,
    notifications: &mpsc::Sender<Notification>,
) -> bool {
    match message {
        Message::Response { id, result } => receive_response(&peer, id, result, frame),
        Message::Request { id, method, params } => {
            receive_request(peer, id, method, params, frame, handler, permits).await
        }
        Message::Notification { method, params } => {
            receive_notification(peer, method, params, frame, handler, notifications).await
        }
    }
}

/// DOC-16 §6、§10：响应资源先建立拥有关系，迟到响应沿载荷析构归还引用。
fn receive_response(peer: &Connection, id: u64, result: Result<Value>, frame: FrameInfo) -> bool {
    let result = match result {
        Ok(value) => {
            match Payload::received(peer, value, frame.wire_bytes, frame.has_resource_tags) {
                Ok(value) => Ok(value),
                Err(problem) => {
                    peer.0.fail(problem.to_string());
                    return false;
                }
            }
        }
        Err(problem) => Err(problem),
    };
    // Account for the wire response before waking its caller. A caller may
    // defer polling without hiding a healthy response from connection health.
    let (reply, stop) = peer.0.claim_pending(
        id,
        Some(result.as_ref().is_err_and(|problem| problem.code == 6)),
    );
    if stop {
        peer.0.fail("插件连续请求超时");
    }
    if let Some(reply) = reply {
        reply.write.cancel();
        let _receiver = reply.sender.send(result);
    }
    true
}

/// DOC-15 CP-3、CP-5：输入容量与编号登记完成后，在独立任务中执行请求。
async fn receive_request(
    peer: Connection,
    id: u64,
    method: String,
    params: Value,
    frame: FrameInfo,
    handler: &Arc<dyn Handler>,
    permits: &Arc<Semaphore>,
) -> bool {
    let params = match Payload::received(&peer, params, frame.wire_bytes, frame.has_resource_tags) {
        Ok(params) => params,
        Err(problem) => {
            peer.0.fail(problem.to_string());
            return false;
        }
    };
    let deadline = if method == "invoke"
        && let Some(value) = parameter_field(params.value(), INVOKE_BUDGET_FIELD)
    {
        let Some(value) = value
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
        else {
            return peer
                .respond(id, &method, Err(error(2, "调用预算需要 u64 毫秒值")))
                .await
                .is_ok();
        };
        peer.0.peer_budget.store(true, Ordering::Release);
        match deadline_after(Duration::from_millis(value)) {
            Ok(deadline) => Some(deadline),
            Err(problem) => return peer.respond(id, &method, Err(problem)).await.is_ok(),
        }
    } else {
        None
    };
    let permit = match permits.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return peer
                .respond(id, &method, Err(error(8, "该进程的在途请求容量已满")))
                .await
                .is_ok();
        }
    };
    let Ok(cancelled) = register_inbound(&peer, id, &method) else {
        return false;
    };
    tokio::spawn(serve_request(InboundRequest {
        peer,
        handler: handler.clone(),
        id,
        method,
        params,
        _permit: permit,
        cancelled,
        deadline,
    }));
    true
}

struct InboundRequest {
    peer: Connection,
    handler: Arc<dyn Handler>,
    id: u64,
    method: String,
    params: Payload,
    _permit: tokio::sync::OwnedSemaphorePermit,
    cancelled: Option<oneshot::Receiver<()>>,
    deadline: Option<Instant>,
}

fn register_inbound(
    peer: &Connection,
    id: u64,
    method: &str,
) -> std::result::Result<Option<oneshot::Receiver<()>>, ()> {
    let (cancel, cancelled) = if method == "invoke" {
        let (send, receive) = oneshot::channel();
        (Some(send), Some(receive))
    } else {
        (None, None)
    };
    let duplicate = {
        let mut inbound = peer.0.inbound.lock();
        if let std::collections::hash_map::Entry::Vacant(entry) = inbound.entry(id) {
            entry.insert(cancel);
            false
        } else {
            true
        }
    };
    if duplicate {
        peer.0.fail("UP/1 在途请求编号重复");
        Err(())
    } else {
        Ok(cancelled)
    }
}

/// DOC-16 §10：invoke响应取消，apply与清理按各自生命周期结束。
async fn serve_request(request: InboundRequest) {
    let InboundRequest {
        peer,
        handler,
        id,
        method,
        params,
        _permit,
        cancelled,
        deadline,
    } = request;
    let label = call_label(&method, params.value());
    let hello = method == "hello";
    let call = async { handler.request(peer.clone(), method, params).await };
    let mut result = match (cancelled, deadline) {
        (Some(cancelled), Some(deadline)) => tokio::select! {
            result = at_invocation_deadline(deadline, call) => result,
            _ = cancelled => Err(error(7, "服务调用已取消")),
        },
        (Some(cancelled), None) => tokio::select! {
            result = call => result,
            _ = cancelled => Err(error(7, "服务调用已取消")),
        },
        _ => call.await,
    };
    if hello && let Ok(reply) = &mut result {
        set_parameter(
            &mut reply.value,
            INVOKE_BUDGET_CAPABILITY,
            Value::Bool(true),
        );
    }
    let removed = peer.0.inbound.lock().remove(&id);
    drop(removed);
    if let Err(problem) = peer.respond(id, &label, result).await {
        peer.0.fail(problem.to_string());
    }
}

/// DOC-16 §6–§8：即时资源簿记先于后续帧，用户通知保持FIFO与等待期限。
async fn receive_notification(
    peer: Connection,
    method: String,
    params: Value,
    frame: FrameInfo,
    handler: &Arc<dyn Handler>,
    notifications: &mpsc::Sender<Notification>,
) -> bool {
    if method == "cancel" {
        return cancel_request(&peer, &params);
    }
    let params = match Payload::received(&peer, params, frame.wire_bytes, frame.has_resource_tags) {
        Ok(params) => params,
        Err(problem) => {
            peer.0.fail(problem.to_string());
            return false;
        }
    };
    if handler.immediate_notification(&method) {
        if let Err(problem) = handler.notify(peer.clone(), method, params).await {
            peer.0.fail(problem.to_string());
            return false;
        }
        return true;
    }
    if notifications
        .try_send((peer.clone(), method, params))
        .is_err()
    {
        peer.0.fail("插件通知队列额度耗尽或已经关闭");
        return false;
    }
    true
}

fn cancel_request(peer: &Connection, params: &Value) -> bool {
    let id = match params {
        Value::Map(fields) => fields.iter().find_map(|(key, value)| {
            (key.as_text() == Some("id"))
                .then(|| value.as_integer().and_then(|id| u64::try_from(id).ok()))
                .flatten()
        }),
        _ => None,
    };
    let Some(id) = id else {
        peer.0.fail("cancel 需要请求编号 id");
        return false;
    };
    let cancellation = peer.0.inbound.lock().get_mut(&id).and_then(Option::take);
    if let Some(cancellation) = cancellation {
        let _finished = cancellation.send(());
    }
    true
}

struct Pending<'a> {
    peer: &'a Connection,
    id: u64,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.peer.cancel_pending(self.id, false);
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

/// CP-8、DOC-16 §3：编码失败的错误说明方向、调用目标与容量数字，不包含载荷内容。
fn encoding_error(problem: crate::CodecError, context: &str) -> WireError {
    let code = match &problem {
        crate::CodecError::Limit(_) | crate::CodecError::Cbor(_) => 8,
        crate::CodecError::Schema(_) | crate::CodecError::TrailingBytes => 2,
    };
    WireError::new(code, format!("{context}编码失败：{problem}"))
}

/// 调用标签：UP/1 方法名；`invoke` 等携带接口方法名的请求附上该名称。
fn call_label(method: &str, params: &Value) -> String {
    let target = match params {
        Value::Map(entries) => entries.iter().find_map(|(key, value)| match (key, value) {
            (Value::Text(key), Value::Text(name)) if key == "method" => Some(name.as_str()),
            _ => None,
        }),
        _ => None,
    };
    match target {
        Some(name) => format!("{method}（方法 {name}）"),
        None => method.to_owned(),
    }
}

/// 按消息种类描述编码失败的位置。
fn describe(message: &Message) -> String {
    match message {
        Message::Request { method, params, .. } => format!("{} 的参数", call_label(method, params)),
        Message::Notification { method, .. } => format!("通知 {method} 的参数"),
        Message::Response {
            id, result: Ok(_), ..
        } => format!("响应 #{id} 的返回值"),
        Message::Response {
            id, result: Err(_), ..
        } => format!("响应 #{id} 的错误详情"),
    }
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
