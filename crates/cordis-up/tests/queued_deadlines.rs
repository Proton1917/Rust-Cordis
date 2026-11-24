use cordis_up::{
    Limits, Message, Value, WireError,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::oneshot,
    time::{Instant, timeout},
};

struct Ignore;
impl Handler for Ignore {
    fn request(
        &self,
        _: Connection,
        _: String,
        _: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        Box::pin(async { Ok(Value::Null.into()) })
    }
    fn notify(
        &self,
        _: Connection,
        _: String,
        _: Payload,
    ) -> HandlerFuture<'_, Result<(), WireError>> {
        Box::pin(async { Ok(()) })
    }
}
struct Gate {
    paused: AtomicBool,
    waker: Mutex<Option<Waker>>,
}
impl Gate {
    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}
struct GatedWriter<W> {
    output: W,
    gate: Arc<Gate>,
    blocked: Option<oneshot::Sender<()>>,
    first_limit: Option<usize>,
}
impl<W: AsyncWrite + Unpin> AsyncWrite for GatedWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.gate.paused.load(Ordering::SeqCst) {
            *this.gate.waker.lock().unwrap() = Some(cx.waker().clone());
            if let Some(blocked) = this.blocked.take() {
                let _ = blocked.send(());
            }
            return Poll::Pending;
        }
        let count = this
            .first_limit
            .map_or(bytes.len(), |limit| limit.min(bytes.len()));
        let result = Pin::new(&mut this.output).poll_write(cx, &bytes[..count]);
        if matches!(result, Poll::Ready(Ok(n)) if n > 0) && this.first_limit.take().is_some() {
            this.gate.paused.store(true, Ordering::SeqCst);
        }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().output).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().output).poll_shutdown(cx)
    }
}
async fn read_message(reader: &mut (impl AsyncRead + Unpin)) -> Message {
    let n = reader.read_u32().await.unwrap() as usize;
    let mut bytes = vec![0; n];
    reader.read_exact(&mut bytes).await.unwrap();
    cordis_up::decode(&bytes, &Limits::default()).unwrap()
}
async fn write_message(writer: &mut (impl AsyncWrite + Unpin), message: Message) {
    let bytes = cordis_up::encode(&message, &Limits::default()).unwrap();
    writer.write_u32(bytes.len() as u32).await.unwrap();
    writer.write_all(&bytes).await.unwrap();
}
fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    value
        .as_map()
        .unwrap()
        .iter()
        .find_map(|(k, v)| (k.as_text() == Some(name)).then_some(v))
        .unwrap()
}
fn fixture(
    paused: bool,
    first_limit: Option<usize>,
) -> (
    Connection,
    tokio::io::DuplexStream,
    Arc<Gate>,
    oneshot::Receiver<()>,
) {
    let (raw, socket) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(socket);
    let gate = Arc::new(Gate {
        paused: AtomicBool::new(paused),
        waker: Mutex::new(None),
    });
    let (blocked, waiting) = oneshot::channel();
    let peer = Connection::attach(
        1,
        read,
        GatedWriter {
            output: write,
            gate: gate.clone(),
            blocked: Some(blocked),
            first_limit,
        },
        ConnectionOptions {
            queue_capacity: 4,
            ..Default::default()
        },
        Arc::new(Ignore),
    )
    .unwrap();
    (peer, raw, gate, waiting)
}
struct DropCount(Arc<AtomicUsize>);
impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn outgoing(commits: &Arc<AtomicUsize>, drops: &Arc<AtomicUsize>) -> Outgoing {
    let commits = commits.clone();
    let guard = DropCount(drops.clone());
    Outgoing::with_commit(
        Value::Map(vec![("method".into(), "record".into())]),
        move || {
            commits.fetch_add(1, Ordering::SeqCst);
            drop(guard);
        },
    )
}

#[tokio::test]
async fn expired_queued_request_is_retracted_and_releases_its_exports() {
    timeout(Duration::from_secs(3), async {
        let (peer, mut raw, gate, waiting) = fixture(true, None);
        peer.notify("prefill", Value::Null).await.unwrap();
        waiting.await.unwrap();
        let commits = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let problem = peer.request("invoke", outgoing(&commits, &drops), Duration::from_millis(20)).await.err().unwrap();
        assert_eq!(problem.code, 6);
        assert_eq!(commits.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        gate.resume();
        peer.notify("barrier", Value::Null).await.unwrap();
        for expected in ["prefill", "barrier"] {
            assert!(matches!(read_message(&mut raw).await, Message::Notification {method,..} if method == expected));
        }
        assert!(peer.is_alive());
        peer.terminate("test complete");
    }).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_retracts_a_writer_owned_frame_before_its_first_byte() {
    timeout(Duration::from_secs(3), async {
        let (peer, mut raw, gate, waiting) = fixture(true, None);
        let commits = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let call_peer = peer.clone();
        let params = outgoing(&commits, &drops);
        let call = tokio::spawn(async move { call_peer.request("invoke", params, Duration::from_secs(1)).await });
        waiting.await.unwrap();
        call.abort();
        assert!(call.await.err().unwrap().is_cancelled());
        assert_eq!(commits.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        gate.resume();
        peer.notify("barrier", Value::Null).await.unwrap();
        assert!(matches!(read_message(&mut raw).await, Message::Notification {method,..} if method == "barrier"));
        peer.terminate("test complete");
    }).await.unwrap();
}

#[tokio::test]
async fn a_partly_written_request_keeps_its_frame_and_then_sends_cancel() {
    timeout(Duration::from_secs(3), async {
        let (peer, mut raw, gate, waiting) = fixture(false, Some(1));
        let commits = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let call_peer = peer.clone();
        let params = outgoing(&commits, &drops);
        let call = tokio::spawn(async move {
            call_peer
                .request("invoke", params, Duration::from_millis(20))
                .await
        });
        waiting.await.unwrap();
        assert_eq!(call.await.unwrap().err().unwrap().code, 6);
        assert_eq!(commits.load(Ordering::SeqCst), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        gate.resume();
        let Message::Request { id, method, .. } = read_message(&mut raw).await else {
            panic!("complete request frame required")
        };
        assert_eq!(method, "invoke");
        let Message::Notification { method, params } = read_message(&mut raw).await else {
            panic!("cancel required")
        };
        assert_eq!(method, "cancel");
        assert_eq!(field(&params, "id"), &Value::from(id));
        assert!(peer.is_alive());
        peer.terminate("test complete");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn queued_invoke_budget_is_updated_at_the_first_write() {
    timeout(Duration::from_secs(3), async {
        let (peer, mut raw, gate, waiting) = fixture(false, None);
        let hello_peer = peer.clone();
        let hello = tokio::spawn(async move { hello_peer.request("hello", Value::Null, Duration::from_secs(1)).await });
        let Message::Request {id, ..} = read_message(&mut raw).await else { panic!("hello required") };
        write_message(&mut raw, Message::Response {id, result: Ok(Value::Map(vec![("invoke_budget_v1".into(), Value::Bool(true))]))}).await;
        hello.await.unwrap().unwrap();
        gate.paused.store(true, Ordering::SeqCst);
        peer.notify("prefill", Value::Null).await.unwrap();
        waiting.await.unwrap();
        let started = Instant::now();
        let mut call = Box::pin(peer.request("invoke", Value::Map(vec![("budget_ms".into(), Value::from(999_u64)), ("method".into(), "record".into())]), Duration::from_millis(500)));
        assert!(call.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        tokio::time::sleep(Duration::from_millis(50)).await;
        gate.resume();
        assert!(matches!(read_message(&mut raw).await, Message::Notification {method,..} if method == "prefill"));
        let Message::Request {id, params, ..} = read_message(&mut raw).await else { panic!("invoke required") };
        let budget = u64::try_from(field(&params, "budget_ms").as_integer().unwrap()).unwrap();
        assert!(budget > 0 && budget <= 455, "remaining budget {budget}");
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert_eq!(params.as_map().unwrap().iter().filter(|(key,_)| key.as_text() == Some("budget_ms")).count(), 1);
        write_message(&mut raw, Message::Response {id, result: Ok(Value::Null)}).await;
        call.await.unwrap();
        peer.terminate("test complete");
    }).await.unwrap();
}
