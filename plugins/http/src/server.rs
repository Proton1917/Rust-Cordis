//! ADR-0038、PR-3/PR-6：专用线程拥有 Tokio、监听套接字及全部 HTTP 连接。

use super::{
    Config,
    registry::Registry,
    socket::{Bridge, SocketState},
};
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use cordis_sdk::{Error, Result};
use std::{
    future::IntoFuture,
    sync::{Arc, mpsc::SyncSender},
    time::Duration,
};
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
    registry: Arc<Registry>,
    bridge: Option<Bridge>,
    binding: Option<super::listen::Binding>,
    context: cordis_sdk::Context,
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
        let routing = Arc::new(super::routes::State {
            registry: registry.clone(),
            context,
            config: config.clone(),
        });
        let prepared = prepare(&config, registry, bridge, stop.clone(), binding).await;
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
        let router = router.layer(axum::middleware::from_fn_with_state(
            routing,
            super::routes::dispatch,
        ));
        serve(listener, router, stop, config.shutdown_ms).await
    })
}

/// DOC-15 §11.3：绑定实际地址并生成本次激活的随机令牌，页面URL使用标准URL编码。
pub(super) async fn prepare(
    config: &Config,
    registry: Arc<Registry>,
    bridge: Option<Bridge>,
    stop: watch::Receiver<bool>,
    binding: Option<super::listen::Binding>,
) -> Result<(tokio::net::TcpListener, Router, WebEntry)> {
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(problem)?;
    let address = listener.local_addr().map_err(problem)?;
    if let Some(binding) = binding {
        binding.save(address)?;
    }
    let token = uuid::Uuid::new_v4().to_string();
    let mut url = url::Url::parse(&format!("http://{address}/")).map_err(problem)?;
    url.set_fragment(Some(&format!("cordis_token={token}")));
    let entry = WebEntry {
        url: url.into(),
        token,
    };
    let router = routes(config, &entry, registry, bridge, stop)?;
    Ok((listener, router, entry))
}

/// ADR-0038：启动信息按激活更新；自定义资源由 tower-http 处理静态文件协议。
fn routes(
    config: &Config,
    entry: &WebEntry,
    registry: Arc<Registry>,
    bridge: Option<Bridge>,
    stop: watch::Receiver<bool>,
) -> Result<Router> {
    let origin = url::Url::parse(&entry.url)
        .map_err(problem)?
        .origin()
        .ascii_serialization();
    let socket = SocketState {
        pages: registry.pages.clone(),
        page_headers: config.page_headers.clone(),
        origin,
        public_origins: config.public_origins.clone(),
        token: entry.token.clone(),
        bridge,
        stop,
    };
    let token = entry.token.clone();
    let entries = registry.clone();
    let assets = registry.clone();
    let router = Router::new()
        .route("/_cordis/socket", get(super::socket::upgrade))
        .with_state(socket)
        .merge(bootstrap_routes(entry)?)
        .route(
            "/_cordis/manifest.json",
            get(move |headers: HeaderMap| {
                let expected = format!("Bearer {token}");
                let entries = entries.clone();
                async move { manifest(headers, &expected, &entries) }
            }),
        )
        .route(
            "/_cordis/{plugin}/{sequence}/{*file}",
            get(move |request| {
                let registry = assets.clone();
                async move { registry.serve(request).await }
            }),
        );
    static_routes(config, registry, router)
}

fn manifest(headers: HeaderMap, expected: &str, entries: &Registry) -> Response {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some(expected)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(entries.entries()),
    )
        .into_response()
}

/// DOC-15 §11：同次激活共享入口信息，浏览器从固定协议地址取得模块与令牌。
fn bootstrap_routes(entry: &WebEntry) -> Result<Router> {
    let bootstrap = format!(
        "globalThis.CORDIS_SESSION = {};\n",
        serde_json::to_string(entry).map_err(problem)?
    );
    let session = entry.clone();
    Ok(Router::new()
        .route(
            "/_cordis/session.json",
            get(move || {
                let entry = session.clone();
                async move { ([(header::CACHE_CONTROL, "no-store")], Json(entry)) }
            }),
        )
        .route(
            "/_cordis/cordis-web.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../assets/cordis-web.js"),
                )
            }),
        )
        .route(
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
        ))
}

/// ADR-0046：应用资源拥有页面外壳；缺省入口使用经过转义的配置标题。
fn static_routes(config: &Config, registry: Arc<Registry>, router: Router) -> Result<Router> {
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
        let title = config
            .title
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let page = DEFAULT_PAGE.replace("{{title}}", &title);
        Ok(router
            .route(
                "/",
                get(move || {
                    let page = page.clone();
                    async move { Html(page) }
                }),
            )
            .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
            .fallback(move |request| {
                let registry = registry.clone();
                async move { registry.serve(request).await }
            }))
    }
}

/// PR-6：先等待 HTTP 连接正常关闭，截止后释放运行时持有的连接任务。
pub(super) async fn serve(
    listener: tokio::net::TcpListener,
    router: Router,
    stop: watch::Receiver<bool>,
    shutdown_ms: u64,
) -> Result<()> {
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
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

const DEFAULT_PAGE: &str = include_str!("../assets/index.html");
