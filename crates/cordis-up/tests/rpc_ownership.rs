use cordis_up::{
    Limits, Message, ObjectRef, Value, WireError,
    payload::InvokeResult,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    sync::{mpsc, oneshot},
    time::timeout,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
type Notice = (String, Value);

struct ResourceHandler(mpsc::UnboundedSender<Notice>);

impl Handler for ResourceHandler {
    fn request(
        &self,
        _: Connection,
        method: String,
        _: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        Box::pin(async move {
            match method.as_str() {
                "resource" => Ok(InvokeResult {
                    value: Value::Array(vec![Value::Bytes(vec![7; 4096]), ObjectRef(42).into()]),
                    refs: vec![42],
                }
                .into_value()
                .into()),
                _ => Err(WireError::new(3, "unknown request")),
            }
        })
    }

    fn notify(
        &self,
        _: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<(), WireError>> {
        Box::pin(async move {
            self.0
                .send((method, params.value().clone()))
                .map_err(|_| WireError::new(4, "notification observer closed"))
        })
    }
}

async fn barrier(peer: &Connection, notices: &mut mpsc::UnboundedReceiver<Notice>) {
    peer.notify("barrier", Value::Null).await.unwrap();
    assert_eq!(
        notices.recv().await.unwrap(),
        ("barrier".into(), Value::Null)
    );
}

fn bytes_pointer(value: &Value) -> *const u8 {
    value.as_array().unwrap()[0].as_bytes().unwrap().as_ptr()
}

#[tokio::test]
async fn taking_received_value_preserves_imported_resource_lifetime() {
    timeout(TEST_TIMEOUT, async {
        let (notices, mut observed) = mpsc::unbounded_channel();
        let handler = Arc::new(ResourceHandler(notices));
        let (caller, provider) = tokio::io::duplex(8192);
        let (read, write) = tokio::io::split(caller);
        let caller = Connection::attach(
            1,
            read,
            write,
            ConnectionOptions::default(),
            handler.clone(),
        )
        .unwrap();
        let (read, write) = tokio::io::split(provider);
        let provider =
            Connection::attach(2, read, write, ConnectionOptions::default(), handler).unwrap();

        let mut payload = caller
            .request("resource", Value::Null, TEST_TIMEOUT)
            .await
            .unwrap();
        let original = payload
            .value()
            .as_map()
            .unwrap()
            .iter()
            .find(|(key, _)| key.as_text() == Some("value"))
            .map(|(_, value)| bytes_pointer(value))
            .unwrap();
        let result = InvokeResult::from_value_owned(payload.take_value()).unwrap();
        assert_eq!(payload.value(), &Value::Null);
        assert_eq!(bytes_pointer(&result.value), original);
        assert_eq!(result.refs, [42]);
        assert_eq!(
            result.value.as_array().unwrap()[1],
            Value::from(ObjectRef(42))
        );
        let object = payload.import(42).unwrap();
        assert_eq!(object.id(), 42);
        assert_eq!(object.peer().id(), caller.id());

        drop(result);
        // FIFO 屏障会观察到此前发送的 release，包括提前释放的错误实现。
        barrier(&caller, &mut observed).await;
        drop(payload);
        barrier(&caller, &mut observed).await;
        drop(object);
        let (method, params) = observed.recv().await.unwrap();
        assert_eq!(method, "release");
        assert_eq!(
            params,
            Value::Map(vec![(
                "objects".into(),
                Value::Array(vec![Value::from(42_u64)]),
            )])
        );
        barrier(&caller, &mut observed).await;
        caller.terminate("test completed");
        provider.terminate("test completed");
    })
    .await
    .expect("resource ownership exchange must complete");
}

thread_local! {
    static WATCHED_ALLOCATION: Cell<(usize, bool)> = const { Cell::new((0, false)) };
}

struct WatchingAllocator;

// SAFETY: System receives the original pointers and layouts. Observation only
// changes allocation-free thread-local cells and never dereferences the pointer.
unsafe impl GlobalAlloc for WatchingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        unsafe { System.realloc(pointer, layout, size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = WATCHED_ALLOCATION.try_with(|watched| {
            if watched.get().0 == pointer as usize {
                watched.set((0, true));
            }
        });
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: WatchingAllocator = WatchingAllocator;

struct AllocationWatch;

impl AllocationWatch {
    fn new(bytes: &[u8]) -> Self {
        assert!(!bytes.is_empty());
        assert_eq!(WATCHED_ALLOCATION.get(), (0, false));
        WATCHED_ALLOCATION.set((bytes.as_ptr() as usize, false));
        Self
    }

    fn released(&self) -> bool {
        WATCHED_ALLOCATION.get().1
    }
}

impl Drop for AllocationWatch {
    fn drop(&mut self) {
        WATCHED_ALLOCATION.set((0, false));
    }
}

struct CommitGuard(Arc<AtomicBool>);

impl Drop for CommitGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct PausedWriter<W> {
    output: W,
    resume: Option<oneshot::Receiver<()>>,
    blocked: Option<oneshot::Sender<()>>,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for PausedWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Some(resume) = this.resume.as_mut() {
            match Pin::new(resume).poll(cx) {
                Poll::Pending => {
                    if let Some(blocked) = this.blocked.take() {
                        let _ = blocked.send(());
                    }
                    return Poll::Pending;
                }
                Poll::Ready(Ok(())) => this.resume = None,
                Poll::Ready(Err(_)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "writer gate closed",
                    )));
                }
            }
        }
        Pin::new(&mut this.output).poll_write(cx, bytes)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().output).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().output).poll_shutdown(cx)
    }
}

async fn read_notification(reader: &mut (impl AsyncRead + Unpin)) -> Notice {
    let size = reader.read_u32().await.unwrap() as usize;
    let mut body = vec![0; size];
    reader.read_exact(&mut body).await.unwrap();
    match cordis_up::decode(&body, &Limits::default()).unwrap() {
        Message::Notification { method, params } => (method, params),
        message => panic!("expected notification, got {message:?}"),
    }
}

#[tokio::test]
async fn queued_notification_releases_value_before_waiting_and_cancels_its_commit() {
    timeout(TEST_TIMEOUT, async {
        let (mut reader, socket) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(socket);
        let (resume, resumed) = oneshot::channel();
        let (blocked, writer_blocked) = oneshot::channel();
        let (notices, _observed) = mpsc::unbounded_channel();
        let peer = Connection::attach(
            1,
            read,
            PausedWriter {
                output: write,
                resume: Some(resumed),
                blocked: Some(blocked),
            },
            ConnectionOptions {
                queue_capacity: 1,
                ..Default::default()
            },
            Arc::new(ResourceHandler(notices)),
        )
        .unwrap();
        peer.notify("tick", Value::from(1_u64)).await.unwrap();
        writer_blocked.await.unwrap();
        peer.notify("tick", Value::from(2_u64)).await.unwrap();

        let committed = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        {
            let bytes = vec![5; 256 * 1024];
            let allocation = AllocationWatch::new(&bytes);
            let did_commit = committed.clone();
            let guard = CommitGuard(released.clone());
            let outgoing = Outgoing::with_commit(Value::Bytes(bytes), move || {
                did_commit.store(true, Ordering::SeqCst);
                drop(guard);
            });
            let mut waiting = Box::pin(peer.notify("cancelled", outgoing));
            assert!(
                waiting
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
            assert!(
                allocation.released(),
                "encoded business value remains owned"
            );
            assert!(!committed.load(Ordering::SeqCst));
            assert!(!released.load(Ordering::SeqCst));
            drop(waiting);
            assert!(!committed.load(Ordering::SeqCst));
            assert!(released.load(Ordering::SeqCst));
        }
        assert!(peer.is_alive());
        resume.send(()).unwrap();
        peer.notify("tick", Value::from(3_u64)).await.unwrap();
        for expected in [1_u64, 2, 3] {
            assert_eq!(
                read_notification(&mut reader).await,
                ("tick".into(), Value::from(expected))
            );
        }
        assert!(peer.is_alive());
        peer.terminate("test completed");
    })
    .await
    .expect("blocked writer and cancellation must complete");
}
