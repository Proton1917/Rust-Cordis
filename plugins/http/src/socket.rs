//! DOC-15 §11.3、DOC-16 §11：WebSocket 终结、Origin/令牌验证与原生转发能力。

use axum::{
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
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
    pub pages: Arc<super::page::Pages>,
    pub page_headers: Vec<String>,
    pub origin: String,
    pub public_origins: Vec<String>,
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
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
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
    if !origin.is_some_and(|origin| {
        origin == state.origin || state.public_origins.iter().any(|allowed| allowed == origin)
    }) || !protocol.split(',').any(|part| part.trim() == expected)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if state.bridge.is_none() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let page = match state.pages.open(&headers, &state.page_headers, peer) {
        Ok(page) => page,
        Err(status) => return status.into_response(),
    };
    websocket
        .protocols(["cordis-up1"])
        .max_frame_size(16 * 1024 * 1024)
        .max_message_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| serve(socket, state, page))
}

/// PR-3、DOC-16 §11：每个升级连接独立占有 Receiver 与有界发送队列。
async fn serve(mut socket: WebSocket, mut state: SocketState, page: super::page::Page) {
    let bridge = state.bridge.take().expect("upgraded forwarding capability");
    let (sender, mut output) = mpsc::channel(64);
    let receiver = match bridge
        .context
        .service::<dyn ForwardReceiver>(Arc::new(Receiver(sender)))
    {
        Ok(receiver) => receiver,
        Err(_) => return,
    };
    let metadata = cordis_sdk::CborValue::Map(vec![(
        "key".into(),
        cordis_sdk::CborValue::Array(
            page.key
                .iter()
                .map(|byte| u64::from(*byte).into())
                .collect(),
        ),
    )]);
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
