//! DOC-15 §11.3、DOC-16 §11：WebSocket 终结、Origin/令牌验证与原生转发能力。

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use cordis_sdk::prelude::*;
use forward_api::{Forward, ForwardReceiver};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

#[derive(Clone)]
pub struct Bridge {
    pub context: Context,
    pub forward: Service<dyn Forward>,
}
#[derive(Clone)]
pub struct SocketState {
    pub origin: String,
    pub token: String,
    pub bridge: Option<Bridge>,
    pub stop: watch::Receiver<bool>,
}
enum Outbound {
    Frame(Vec<u8>),
    Close,
}
struct Receiver(mpsc::Sender<Outbound>);

#[cordis_sdk::interface]
impl ForwardReceiver for Receiver {
    async fn frame(&self, frame: Vec<u8>) -> Result<()> {
        self.0
            .try_send(Outbound::Frame(frame))
            .map_err(|problem| Error::new(8, problem.to_string()))
    }
    async fn close(&self, _reason: String) -> Result<()> {
        self.0
            .try_send(Outbound::Close)
            .map_err(|problem| Error::new(4, problem.to_string()))
    }
}

/// DOC-15 §11.3：Origin 与本次激活令牌在升级边界同时验证。
pub async fn upgrade(
    State(state): State<SocketState>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let protocol = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let expected = format!("cordis-token.{}", state.token);
    if origin != Some(state.origin.as_str())
        || !protocol.split(',').any(|part| part.trim() == expected)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if state.bridge.is_none() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    websocket
        .protocols(["cordis-up1"])
        .max_frame_size(16 * 1024 * 1024)
        .max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| serve(socket, state))
}

/// PR-3、DOC-16 §11：每个升级连接独立占有 Receiver 与有界发送队列。
async fn serve(mut socket: WebSocket, mut state: SocketState) {
    let bridge = state.bridge.take().expect("upgraded forwarding capability");
    let (sender, mut output) = mpsc::channel(64);
    let receiver = match bridge
        .context
        .service::<dyn ForwardReceiver>(Arc::new(Receiver(sender)))
    {
        Ok(receiver) => receiver,
        Err(_) => return,
    };
    let metadata = cordis_sdk::CborValue::Map(vec![("origin".into(), state.origin.into())]);
    let channel = match bridge.forward.open(metadata, receiver).await {
        Ok(channel) => channel,
        Err(_) => return,
    };
    loop {
        tokio::select! {
            received = socket.recv() => match received {
                Some(Ok(Message::Binary(bytes))) => if bridge.forward.frame(channel, bytes.to_vec()).await.is_err() { break; },
                Some(Ok(Message::Ping(bytes))) => if socket.send(Message::Pong(bytes)).await.is_err() { break; },
                Some(Ok(Message::Pong(_))) => {},
                _ => break,
            },
            item = output.recv() => match item {
                Some(Outbound::Frame(bytes)) => if socket.send(Message::Binary(bytes.into())).await.is_err() { break; },
                Some(Outbound::Close) | None => break,
            },
            _ = state.stop.changed() => break,
        }
    }
    let _closed = bridge.forward.close(channel, "浏览器连接结束".into()).await;
    let _closed = socket.send(Message::Close(None)).await;
}
