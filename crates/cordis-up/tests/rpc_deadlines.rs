use cordis_up::{
    Value, WireError,
    rpc::{
        Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload, bound_invocation,
    },
};
use std::{sync::Arc, time::Duration};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const REMOTE_TIMEOUT: &str = "peer deadline expired";

struct Replies;

impl Handler for Replies {
    fn request(
        &self,
        _: Connection,
        method: String,
        _: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        Box::pin(async move {
            match method.as_str() {
                "invoke" => std::future::pending().await,
                "timeout" => Err(WireError::new(6, REMOTE_TIMEOUT)),
                "ok" => Ok(Value::Null.into()),
                _ => Err(WireError::new(2, "peer rejected the request")),
            }
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

struct Peers {
    caller: Connection,
    provider: Connection,
}

impl Peers {
    fn new() -> Self {
        Self::with_handler(Arc::new(Replies))
    }

    fn with_handler(handler: Arc<dyn Handler>) -> Self {
        let (caller, provider) = tokio::io::duplex(4096);
        let mut options = ConnectionOptions {
            consecutive_timeouts: 3,
            ..Default::default()
        };
        options.limits.max_frame = 1024;
        let (read, write) = tokio::io::split(caller);
        let caller = Connection::attach(1, read, write, options.clone(), handler.clone()).unwrap();
        let (read, write) = tokio::io::split(provider);
        let provider = Connection::attach(2, read, write, options, handler).unwrap();
        Self { caller, provider }
    }

    async fn remote_timeout(&self) {
        let error = self
            .caller
            .request("timeout", Value::Null, REQUEST_TIMEOUT)
            .await
            .err()
            .expect("the peer returns a timeout response");
        assert_eq!(error.code, 6);
        assert_eq!(error.message, REMOTE_TIMEOUT);
    }

    async fn assert_timeout_threshold_reached(&self) {
        assert!(
            !self.caller.is_alive(),
            "three consecutive requests with their own deadline must close the connection"
        );
        assert_eq!(self.caller.closed().await, "插件连续请求超时");
    }
}

impl Drop for Peers {
    fn drop(&mut self) {
        self.caller.terminate("test completed");
        self.provider.terminate("test completed");
    }
}

#[tokio::test]
async fn remote_timeout_responses_reach_the_connection_threshold() {
    let peers = Peers::new();
    for _ in 0..2 {
        peers.remote_timeout().await;
        assert!(peers.caller.is_alive());
    }
    peers.remote_timeout().await;
    peers.assert_timeout_threshold_reached().await;
}

#[tokio::test]
async fn local_and_remote_timeouts_share_one_streak() {
    let peers = Peers::new();
    for _ in 0..2 {
        let problem = peers
            .caller
            .request("invoke", Value::Null, Duration::from_millis(10))
            .await
            .err()
            .unwrap();
        assert_eq!(problem.code, 6);
        assert!(peers.caller.is_alive(), "each local deadline counts once");
    }
    peers.remote_timeout().await;
    peers.assert_timeout_threshold_reached().await;
}

async fn assert_reply_resets_the_streak(method: &str, error_code: Option<u32>) {
    let peers = Peers::new();
    for _ in 0..2 {
        peers.remote_timeout().await;
    }
    let result = peers
        .caller
        .request(method, Value::Null, REQUEST_TIMEOUT)
        .await;
    match error_code {
        Some(code) => assert_eq!(result.err().unwrap().code, code),
        None => assert_eq!(result.unwrap().value(), &Value::Null),
    }
    for _ in 0..2 {
        peers.remote_timeout().await;
        assert!(peers.caller.is_alive(), "a normal reply resets the streak");
    }
    peers.remote_timeout().await;
    peers.assert_timeout_threshold_reached().await;
}

#[tokio::test]
async fn successful_replies_reset_the_timeout_streak() {
    assert_reply_resets_the_streak("ok", None).await;
}

#[tokio::test]
async fn non_timeout_error_replies_reset_the_timeout_streak() {
    assert_reply_resets_the_streak("rejected", Some(2)).await;
}

#[tokio::test]
async fn inherited_timeout_replies_do_not_count_or_reset_the_own_timeout_streak() {
    let peers = Peers::new();
    for _ in 0..2 {
        peers.remote_timeout().await;
    }
    bound_invocation(Duration::from_secs(5), async {
        for _ in 0..4 {
            peers.remote_timeout().await;
            assert!(
                peers.caller.is_alive(),
                "inherited deadline responses must not penalize the connection"
            );
        }
        Ok(())
    })
    .await
    .unwrap();
    peers.remote_timeout().await;
    peers.assert_timeout_threshold_reached().await;
}

#[tokio::test]
async fn local_encoding_errors_do_not_reset_the_timeout_streak() {
    let peers = Peers::new();
    for _ in 0..2 {
        peers.remote_timeout().await;
    }
    let error = peers
        .caller
        .request("ok", Value::Bytes(vec![0; 2048]), REQUEST_TIMEOUT)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, 8);
    assert!(peers.caller.is_alive());
    peers.remote_timeout().await;
    peers.assert_timeout_threshold_reached().await;
}

struct ObservedReplies(Arc<tokio::sync::Notify>);

impl Handler for ObservedReplies {
    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        if method == "ok" {
            let written = self.0.clone();
            Box::pin(async move {
                Ok(Outgoing::new(Value::Null).after_write(move || written.notify_one()))
            })
        } else {
            Replies.request(peer, method, params)
        }
    }

    fn notify(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<(), WireError>> {
        Replies.notify(peer, method, params)
    }
}

#[tokio::test]
async fn received_success_resets_streak_before_the_caller_is_repolled() {
    use std::{
        future::Future,
        task::{Context, Waker},
    };

    tokio::time::timeout(Duration::from_secs(5), async {
        let written = Arc::new(tokio::sync::Notify::new());
        let peers = Peers::with_handler(Arc::new(ObservedReplies(written.clone())));
        for _ in 0..2 {
            peers.remote_timeout().await;
        }

        let mut deferred = Box::pin(peers.caller.request("ok", Value::Null, REQUEST_TIMEOUT));
        assert!(
            deferred
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        written.notified().await;
        // This reverse request follows the success on the same wire. Its reply
        // proves the caller has received the success without polling deferred.
        let barrier = peers
            .provider
            .request("barrier", Value::Null, REQUEST_TIMEOUT)
            .await;
        assert_eq!(barrier.err().unwrap().code, 2);

        peers.remote_timeout().await;
        assert!(
            peers.caller.is_alive(),
            "a received success must break the timeout streak"
        );
        assert_eq!(deferred.await.unwrap().value(), &Value::Null);
    })
    .await
    .expect("wire-order exchange must complete");
}
