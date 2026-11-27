use super::*;
use cordis_up::rpc::{ConnectionOptions, Handler, HandlerFuture, Payload};
use std::sync::atomic::{AtomicUsize, Ordering};

struct WriteGate {
    permits: AtomicUsize,
    bytes: AtomicUsize,
    blocked: tokio::sync::Notify,
    waiter: Mutex<Option<std::task::Waker>>,
}

struct GatedWriter(Arc<WriteGate>);

impl tokio::io::AsyncWrite for GatedWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if self
            .0
            .permits
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            })
            .is_ok()
        {
            let written = bytes.len().min(1);
            self.0.bytes.fetch_add(written, Ordering::SeqCst);
            return std::task::Poll::Ready(Ok(written));
        }
        *self.0.waiter.lock() = Some(cx.waker().clone());
        self.0.blocked.notify_one();
        std::task::Poll::Pending
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

struct Idle;
impl Handler for Idle {
    fn request(&self, _: Connection, _: String, _: Payload) -> HandlerFuture<'_, Result<Outgoing>> {
        Box::pin(async { Ok(Value::Null.into()) })
    }
    fn notify(&self, _: Connection, _: String, _: Payload) -> HandlerFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

fn peer(id: u64) -> (Connection, tokio::io::DuplexStream) {
    let (local, remote) = tokio::io::duplex(64);
    let (read, write) = tokio::io::split(local);
    (
        Connection::attach(
            id,
            read,
            write,
            ConnectionOptions::default(),
            Arc::new(Idle),
        )
        .unwrap(),
        remote,
    )
}

struct Route {
    lifetime: Option<Retirement>,
    dropped: Arc<AtomicUsize>,
    directory: Weak<Directory<Route>>,
}
impl RouteLifetime for Route {
    fn retirement(&self) -> Option<Retirement> {
        self.lifetime.clone()
    }
}
impl Drop for Route {
    fn drop(&mut self) {
        if let Some(directory) = self.directory.upgrade() {
            assert!(
                directory.entries.try_lock().is_some(),
                "route destructor ran inside directory lock"
            );
        }
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

fn export(
    router: &Router<Route>,
    peer: &Connection,
    owner: Option<u64>,
    lifetime: Option<Retirement>,
    dropped: &Arc<AtomicUsize>,
    batch: &mut Batch<Route>,
) -> u64 {
    router
        .export(
            peer,
            Arc::new(Route {
                lifetime,
                dropped: dropped.clone(),
                directory: Arc::downgrade(&router.directory),
            }),
            owner,
            batch,
        )
        .unwrap()
}

#[tokio::test]
async fn outgoing_batch_cancellation_observes_first_positive_write() {
    for written_before_cancel in [false, true] {
        let router = Router::new(8, Duration::from_secs(1));
        let dropped = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(WriteGate {
            permits: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            blocked: tokio::sync::Notify::new(),
            waiter: Mutex::new(None),
        });
        let (local, _remote) = tokio::io::duplex(64);
        let (read, _keep_read_open) = tokio::io::split(local);
        let recipient = Connection::attach(
            17,
            read,
            GatedWriter(gate.clone()),
            ConnectionOptions::default(),
            Arc::new(Idle),
        )
        .unwrap();
        let mut batch = router.batch(recipient.id());
        let id = export(&router, &recipient, Some(9), None, &dropped, &mut batch);
        let outgoing = batch.outgoing(Value::Null);
        let request_peer = recipient.clone();
        let request = tokio::spawn(async move {
            request_peer
                .request("probe", outgoing, Duration::from_secs(60))
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), gate.blocked.notified())
            .await
            .unwrap();
        assert_eq!(gate.bytes.load(Ordering::SeqCst), 0);
        if written_before_cancel {
            gate.permits.store(1, Ordering::SeqCst);
            gate.waiter.lock().take().unwrap().wake();
            // The second blocked write occurs after the first-write callback.
            tokio::time::timeout(Duration::from_secs(2), gate.blocked.notified())
                .await
                .unwrap();
            assert_eq!(gate.bytes.load(Ordering::SeqCst), 1);
        }
        request.abort();
        assert!(matches!(request.await, Err(problem) if problem.is_cancelled()));
        assert_eq!(
            router.route(recipient.id(), id).is_ok(),
            written_before_cancel
        );
        assert_eq!(
            dropped.load(Ordering::Relaxed),
            usize::from(!written_before_cancel)
        );
        if written_before_cancel {
            router.release(recipient.id(), &[id]).unwrap();
            assert_eq!(dropped.load(Ordering::Relaxed), 1);
        }
        recipient.terminate("test finished");
    }
}

#[tokio::test]
async fn source_reclaim_preserves_unrelated_routes_and_numbering() {
    for unrelated in [1, 100, 10_000] {
        let router = Router::new(20_000, Duration::from_secs(1));
        let (recipient, _remote) = peer(2);
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut batch = router.batch(2);
        let first = export(&router, &recipient, Some(1), None, &dropped, &mut batch);
        for _ in 0..unrelated {
            export(&router, &recipient, Some(9), None, &dropped, &mut batch);
        }
        let last = export(&router, &recipient, Some(1), None, &dropped, &mut batch);
        router.reclaim(1);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        assert!(router.route(2, first).is_err());
        assert!(router.route(2, last).is_err());
        assert!(router.route(2, first + 1).is_ok());
        let next = export(&router, &recipient, None, None, &dropped, &mut batch);
        assert_eq!(next, last + 1);
        router.release(2, &[first, last]).unwrap();
        let entries = router.directory.entries.lock();
        assert!(!entries.owners.contains_key(&1));
        assert_eq!(entries.owners[&9].len(), unrelated);
        assert_eq!(entries.removed, 2);
        drop(entries);
        recipient.terminate("test finished");
    }
}

#[tokio::test]
async fn release_retire_rollback_and_both_connection_ends_unlink_every_index() {
    let router = Router::new(20, Duration::from_secs(1));
    let (first, _first_remote) = peer(1);
    let (second, _second_remote) = peer(2);
    let dropped = Arc::new(AtomicUsize::new(0));
    let lifetime = Retirement::default();
    let mut first_batch = router.batch(1);
    let mut second_batch = router.batch(2);
    let own = export(
        &router,
        &first,
        Some(1),
        Some(lifetime.clone()),
        &dropped,
        &mut first_batch,
    );
    let forwarded = export(
        &router,
        &second,
        Some(1),
        Some(lifetime.clone()),
        &dropped,
        &mut second_batch,
    );
    let released = export(
        &router,
        &second,
        Some(8),
        Some(lifetime.clone()),
        &dropped,
        &mut second_batch,
    );
    router.release(2, &[released]).unwrap();
    router.reclaim(1);
    assert_eq!(dropped.load(Ordering::Relaxed), 3);
    assert!(router.route(1, own).is_err());
    assert!(router.route(2, forwarded).is_err());
    router.retire(&lifetime);
    router.reclaim(2);
    drop((first_batch, second_batch));
    {
        let entries = router.directory.entries.lock();
        assert!(entries.recipients.is_empty());
        assert!(entries.lifetimes.is_empty());
        assert!(entries.owners.is_empty());
    }

    let (third, _third_remote) = peer(3);
    let next_lifetime = Retirement::default();
    let mut batch = router.batch(3);
    let id = export(
        &router,
        &third,
        Some(3),
        Some(next_lifetime.clone()),
        &dropped,
        &mut batch,
    );
    router.retire(&next_lifetime);
    assert!(router.route(3, id).is_err());
    assert!(router.directory.entries.lock().owners.is_empty());
    router.release(3, &[id]).unwrap();
    drop(batch);
    let mut cancelled = router.batch(3);
    export(&router, &third, Some(4), None, &dropped, &mut cancelled);
    drop(cancelled);
    assert!(router.directory.entries.lock().owners.is_empty());
    assert_eq!(dropped.load(Ordering::Relaxed), 5);
    first.terminate("test finished");
    second.terminate("test finished");
    third.terminate("test finished");
}
