use cordis_up::{
    Limits, Message, Value, WireError,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    sync::Notify,
    time::timeout,
};

struct Replies {
    received: Arc<Notify>,
    written: Arc<Notify>,
}
impl Handler for Replies {
    fn request(
        &self,
        _: Connection,
        _: String,
        _: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        let received = self.received.clone();
        let written = self.written.clone();
        Box::pin(async move {
            received.notify_one();
            Ok(Outgoing::new(Value::Null).after_write(move || written.notify_one()))
        })
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
fn frame(message: Message) -> Vec<u8> {
    let bytes = cordis_up::encode(&message, &Limits::default()).unwrap();
    let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
    frame.extend(bytes);
    frame
}
async fn read_message(raw: &mut DuplexStream, first: Option<u8>) -> Message {
    let size = if let Some(first) = first {
        let mut header = [first, 0, 0, 0];
        raw.read_exact(&mut header[1..]).await.unwrap();
        u32::from_be_bytes(header)
    } else {
        raw.read_u32().await.unwrap()
    } as usize;
    let mut bytes = vec![0; size];
    raw.read_exact(&mut bytes).await.unwrap();
    cordis_up::decode(&bytes, &Limits::default()).unwrap()
}
struct DropCount(Arc<AtomicUsize>);
impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn data_byte_limit_preserves_reserved_controls_and_fifo() {
    timeout(Duration::from_secs(5), async {
        let (mut raw, socket) = tokio::io::duplex(1);
        let (read, write) = tokio::io::split(socket);
        let received = Arc::new(Notify::new());
        let written = Arc::new(Notify::new());
        let peer = Connection::attach(1, read, write, ConnectionOptions {
            queue_capacity: 1, max_buffered_bytes: 4096, control_reserve_bytes: 256,
            control_queue_capacity: 2, ..Default::default()
        }, Arc::new(Replies { received: received.clone(), written: written.clone() })).unwrap();
        peer.notify("first", Value::Bytes(vec![1; 1500])).await.unwrap();
        let first = raw.read_u8().await.unwrap();
        peer.notify("second", Value::Bytes(vec![2; 1500])).await.unwrap();
        assert_eq!(peer.buffered_bytes(), 4096);
        let commits = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let commit = commits.clone();
        let guard = DropCount(drops.clone());
        let rejected = peer.notify("overload", Outgoing::with_commit(Value::Bytes(vec![3; 2000]), move || {
            commit.fetch_add(1, Ordering::SeqCst); drop(guard);
        })).await.unwrap_err();
        assert_eq!(rejected.code, 8);
        assert_eq!(commits.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(peer.is_alive());
        peer.try_notify("release", Value::Map(vec![("objects".into(), Value::Array(vec![Value::from(7_u64)]))])).unwrap();
        raw.write_all(&frame(Message::Request {id: 99, method: "probe".into(), params: Value::Null})).await.unwrap();
        received.notified().await;
        assert_eq!(peer.buffered_bytes(), 4352);
        assert_eq!(peer.peak_buffered_bytes(), 4352);
        for (index, expected) in ["first", "second", "release"].iter().enumerate() {
            assert!(matches!(read_message(&mut raw, (index == 0).then_some(first)).await, Message::Notification {method, ..} if method == *expected));
        }
        assert!(matches!(read_message(&mut raw, None).await, Message::Response {id: 99, result: Ok(Value::Null)}));
        written.notified().await;
        assert_eq!(peer.buffered_bytes(), 0);
        assert!(peer.is_alive());
        peer.terminate("test complete");
    }).await.unwrap();
}

struct Reverse;
impl Handler for Reverse {
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
        peer: Connection,
        method: String,
        _: Payload,
    ) -> HandlerFuture<'_, Result<(), WireError>> {
        Box::pin(async move {
            if method == "first" {
                peer.request("reverse", Value::Null, Duration::from_secs(2))
                    .await?;
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn full_notification_queue_retires_without_waiting_for_a_reverse_reply() {
    timeout(Duration::from_secs(3), async {
        let (mut raw, socket) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(socket);
        let peer = Connection::attach(
            1,
            read,
            write,
            ConnectionOptions {
                queue_capacity: 1,
                request_timeout: Duration::from_secs(2),
                ..Default::default()
            },
            Arc::new(Reverse),
        )
        .unwrap();
        raw.write_all(&frame(Message::Notification {
            method: "first".into(),
            params: Value::Null,
        }))
        .await
        .unwrap();
        let Message::Request { id, method, .. } = read_message(&mut raw, None).await else {
            panic!("reverse request required")
        };
        assert_eq!(method, "reverse");
        let mut input = frame(Message::Notification {
            method: "second".into(),
            params: Value::Null,
        });
        input.extend(frame(Message::Notification {
            method: "third".into(),
            params: Value::Null,
        }));
        input.extend(frame(Message::Response {
            id,
            result: Ok(Value::Null),
        }));
        raw.write_all(&input).await.unwrap();
        let reason = timeout(Duration::from_millis(250), peer.closed())
            .await
            .unwrap();
        assert!(reason.contains("通知队列额度耗尽"), "{reason}");
    })
    .await
    .unwrap();
}
