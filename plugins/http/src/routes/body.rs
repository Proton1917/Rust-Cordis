//! ADR-0046 §3.4/3.8：请求流按需读取，响应流由 Hyper 的拉取节奏驱动。

use super::super::Config;
use axum::{
    body::{Body, BodyDataStream, Bytes},
    http::StatusCode,
};
use cordis_sdk::{Context, Error, Result, Service};
use futures_util::StreamExt;
use http_api::Body as Payload;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
    time::{Duration, Instant},
};
use stream_api::ByteStream;
use tokio::sync::{Mutex, watch};

struct Input {
    stream: BodyDataStream,
    pending: Bytes,
    received: u64,
    last: Instant,
}

pub(super) struct Upload {
    input: Mutex<Option<Input>>,
    error: AtomicU16,
    closed: watch::Sender<bool>,
    limit: u64,
    chunk: u32,
    idle: Duration,
    runtime: tokio::runtime::Handle,
}

/// ADR-0046 §3.4：请求退出和取消都会结束上传读取，正在等待的套接字读取收到关闭信号。
pub(super) struct UploadGuard(pub Arc<Upload>);
impl Drop for UploadGuard {
    fn drop(&mut self) {
        self.0.closed.send_replace(true);
        if let Ok(mut input) = self.0.input.try_lock() {
            input.take();
        }
    }
}

impl Upload {
    pub fn status(&self) -> Option<StatusCode> {
        StatusCode::from_u16(self.error.load(Ordering::Acquire)).ok()
    }

    fn failure(&self, status: StatusCode, problem: impl std::fmt::Display) -> Error {
        self.error.store(status.as_u16(), Ordering::Release);
        Error::new(
            if status == StatusCode::GATEWAY_TIMEOUT {
                6
            } else {
                8
            },
            problem.to_string(),
        )
    }

    async fn next(&self, input: &mut Input) -> Result<bool> {
        let mut closed = self.closed.subscribe();
        if *closed.borrow() {
            return Err(Error::new(4, "HTTP 请求流已关闭"));
        }
        if input.last.elapsed() > self.idle {
            return Err(self.failure(StatusCode::GATEWAY_TIMEOUT, "HTTP 请求流空闲超时"));
        }
        let frame = tokio::select! {
            _ = closed.changed() => return Err(Error::new(4, "HTTP 请求流已关闭")),
            frame = tokio::time::timeout(self.idle, input.stream.next()) => {
                frame.map_err(|_| self.failure(StatusCode::GATEWAY_TIMEOUT, "HTTP 请求体读取超时"))?
            },
        };
        match frame {
            Some(Ok(bytes)) => {
                input.received += bytes.len() as u64;
                if input.received > self.limit {
                    return Err(
                        self.failure(StatusCode::PAYLOAD_TOO_LARGE, "HTTP 请求体超过路由上限")
                    );
                }
                input.pending = bytes;
                Ok(true)
            }
            Some(Err(problem)) => Err(self.failure(StatusCode::BAD_REQUEST, problem)),
            None => Ok(false),
        }
    }
}

#[cordis_sdk::interface]
impl ByteStream for Upload {
    async fn read(&self, max_bytes: u32) -> Result<Option<Vec<u8>>> {
        let mut operation = Box::pin(self.read_local(max_bytes));
        futures_util::future::poll_fn(|context| {
            let _entered = self.runtime.enter();
            operation.as_mut().poll(context)
        })
        .await
    }
}

impl Upload {
    /// ADR-0046 §3.4：远端轮询进入流所属的 Tokio 上下文，套接字仍由 HTTP 运行时驱动。
    async fn read_local(&self, max_bytes: u32) -> Result<Option<Vec<u8>>> {
        let count = stream_api::read_size(max_bytes)?.min(self.chunk as usize);
        if let Some(status) = self.status() {
            return Err(Error::new(8, format!("HTTP 请求流已结束：{status}")));
        }
        let mut state = self.input.lock().await;
        let Some(input) = state.as_mut() else {
            return Ok(None);
        };
        while input.pending.is_empty() {
            match self.next(input).await {
                Ok(true) => {}
                Ok(false) => {
                    state.take();
                    return Ok(None);
                }
                Err(problem) => {
                    state.take();
                    return Err(problem);
                }
            }
        }
        input.last = Instant::now();
        let count = count.min(input.pending.len());
        Ok(Some(input.pending.split_to(count).to_vec()))
    }
}

/// ADR-0046 §3.4：已知小请求收集到内联值，其余以拥有型字节流交给处理方。
pub(super) async fn prepare(
    context: &Context,
    body: Body,
    length: Option<u64>,
    config: &Config,
    limit: u64,
) -> std::result::Result<(Payload, UploadGuard), StatusCode> {
    if length.is_some_and(|length| length > limit) {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let upload = Arc::new(Upload {
        input: Mutex::new(Some(Input {
            stream: body.into_data_stream(),
            pending: Bytes::new(),
            received: 0,
            last: Instant::now(),
        })),
        error: AtomicU16::new(0),
        closed: watch::channel(false).0,
        limit,
        chunk: config.stream_chunk_bytes,
        idle: Duration::from_millis(config.body_idle_ms),
        runtime: tokio::runtime::Handle::current(),
    });
    let guard = UploadGuard(upload.clone());
    let body = if length.is_some_and(|length| length <= config.inline_body_bytes) {
        let mut bytes = Vec::with_capacity(length.unwrap_or(0) as usize);
        while let Some(chunk) = upload
            .read(config.stream_chunk_bytes)
            .await
            .map_err(|_| upload.status().unwrap_or(StatusCode::BAD_REQUEST))?
        {
            bytes.extend(chunk);
        }
        if bytes.is_empty() {
            Payload::Empty
        } else {
            Payload::Inline(bytes.into())
        }
    } else {
        Payload::Stream {
            length,
            source: context
                .service::<dyn ByteStream>(upload)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        }
    };
    Ok((body, guard))
}

/// ADR-0046 §3.4：处理方提前完成时仍计量余下上传，响应头提交前确定 413。
pub(super) async fn finish(upload: &Upload, chunk: u32) -> std::result::Result<(), StatusCode> {
    while upload
        .read(chunk)
        .await
        .map_err(|_| upload.status().unwrap_or(StatusCode::BAD_REQUEST))?
        .is_some()
    {}
    Ok(())
}

pub(super) fn response(body: Payload, config: &Config) -> (Body, Option<u64>) {
    match body {
        Payload::Empty => (Body::empty(), Some(0)),
        Payload::Inline(bytes) => {
            let size = bytes.len() as u64;
            (Body::from(bytes.0), Some(size))
        }
        Payload::Stream { length, source } => (download(source, config), length),
    }
}

/// ADR-0046 §3.4：下游断连时 Hyper 丢弃响应体，源对象引用随流状态归还。
fn download(source: Service<dyn ByteStream>, config: &Config) -> Body {
    let chunk = config.stream_chunk_bytes;
    let idle = Duration::from_millis(config.body_idle_ms);
    let stream = futures_util::stream::try_unfold(
        (source, Instant::now()),
        move |(source, last)| async move {
            if last.elapsed() > idle {
                return Err(Error::new(6, "HTTP 响应流空闲超时"));
            }
            match tokio::time::timeout(idle, source.read(chunk))
                .await
                .map_err(|_| Error::new(6, "HTTP 响应流读取超时"))??
            {
                None => Ok(None),
                Some(bytes) if bytes.is_empty() || bytes.len() > chunk as usize => {
                    Err(Error::new(2, "响应流块长度超出读取契约"))
                }
                Some(bytes) => Ok(Some((Bytes::from(bytes), (source, Instant::now())))),
            }
        },
    );
    Body::from_stream(stream)
}
