//! ADR-0038、PR-3/PR-6：专用线程拥有 Tokio、监听套接字及全部 HTTP 连接。

use super::Config;
use axum::{Router, http::header, response::Html, routing::get};
use cordis_sdk::{Error, Result};
use std::{future::IntoFuture, sync::mpsc::SyncSender, time::Duration};
use tokio::sync::watch;
use tower_http::services::{ServeDir, ServeFile};
use web_api::WebEntry;

pub(super) fn problem(error: impl std::fmt::Display) -> Error {
    Error::new(1, error.to_string())
}

/// PR-3：运行时在专用线程创建和释放，退出前等待连接任务及阻塞文件任务结束。
pub(super) fn run(
    config: Config,
    stop: watch::Receiver<bool>,
    ready: SyncSender<Result<WebEntry>>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(problem);
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(error) => {
            let _closed = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    runtime.block_on(async move {
        let prepared = prepare(&config).await;
        let (listener, router, entry) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let _closed = ready.send(Err(error.clone()));
                return Err(error);
            }
        };
        ready
            .send(Ok(entry))
            .map_err(|_| Error::new(7, "HTTP启动接收方已结束"))?;
        serve(listener, router, stop, config.shutdown_ms).await
    })
}

/// DOC-15 §11.3：绑定实际地址并生成本次激活的随机令牌，页面URL使用标准URL编码。
async fn prepare(config: &Config) -> Result<(tokio::net::TcpListener, Router, WebEntry)> {
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(problem)?;
    let address = listener.local_addr().map_err(problem)?;
    let token = uuid::Uuid::new_v4().to_string();
    let mut url = url::Url::parse(&format!("http://{address}/")).map_err(problem)?;
    url.set_fragment(Some(&format!("cordis_token={token}")));
    let entry = WebEntry {
        url: url.into(),
        token,
    };
    let router = routes(config, &entry)?;
    Ok((listener, router, entry))
}

/// ADR-0038：启动信息按激活更新；自定义资源由 tower-http 处理静态文件协议。
fn routes(config: &Config, entry: &WebEntry) -> Result<Router> {
    let bootstrap = format!(
        "globalThis.CORDIS_SESSION = {};\n",
        serde_json::to_string(entry).map_err(problem)?
    );
    let router = Router::new().route(
        "/_cordis/session.js",
        get(move || {
            let body = bootstrap.clone();
            async move {
                (
                    [
                        (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                        (header::CACHE_CONTROL, "no-store"),
                    ],
                    body,
                )
            }
        }),
    );
    if let Some(root) = &config.assets {
        let root = root.canonicalize().map_err(problem)?;
        let index = root.join(&config.index).canonicalize().map_err(problem)?;
        if !root.is_dir() || !index.is_file() || !index.starts_with(&root) {
            return Err(Error::new(2, "assets需要为目录，index需要位于该目录内"));
        }
        Ok(router
            .route_service("/", ServeFile::new(index))
            .fallback_service(ServeDir::new(root)))
    } else {
        Ok(router.route("/", get(|| async { Html(DEFAULT_PAGE) })))
    }
}

/// PR-6：先等待 HTTP 连接正常关闭，截止后释放运行时持有的连接任务。
async fn serve(
    listener: tokio::net::TcpListener,
    router: Router,
    stop: watch::Receiver<bool>,
    shutdown_ms: u64,
) -> Result<()> {
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(stopping(stop.clone()))
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result.map_err(problem),
        _ = stopping(stop) => {
            tokio::time::timeout(Duration::from_millis(shutdown_ms), &mut server).await
                .map_err(|_| Error::new(6, "HTTP连接关闭超过截止时间"))?.map_err(problem)
        }
    }
}

async fn stopping(mut stop: watch::Receiver<bool>) {
    while !*stop.borrow_and_update() {
        if stop.changed().await.is_err() {
            return;
        }
    }
}

const DEFAULT_PAGE: &str = "<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Cordis 窗口</title><script src=\"/_cordis/session.js\"></script><body><h1>Cordis 窗口</h1><p>运行中</p></body></html>";
