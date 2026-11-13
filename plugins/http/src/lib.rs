//! DOC-15 §11.3、ADR-0038：原生 HTTP 插件提供静态资源、页面地址和激活令牌。
#![forbid(unsafe_code)]

mod listen;
mod page;
mod registry;
mod routes;
mod server;
mod socket;
#[cfg(test)]
mod tests;

use cordis_sdk::prelude::*;
use forward_api::Forward;
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Component as PathComponent, PathBuf},
    sync::Mutex,
};
use tokio::sync::{oneshot, watch};
use web_api::{Web, WebEntry, WebRegistration};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub title: String,
    #[serde(deserialize_with = "listen_address")]
    pub listen: SocketAddr,
    pub assets: Option<PathBuf>,
    pub index: PathBuf,
    pub shutdown_ms: u64,
    pub inline_body_bytes: u64,
    pub max_body_bytes: u64,
    pub stream_chunk_bytes: u32,
    pub route_timeout_ms: u64,
    pub body_idle_ms: u64,
    pub replace_grace_ms: u64,
    pub public_origins: Vec<String>,
    pub page_headers: Vec<String>,
}

/// DOC-15 §11.3：部署配置中的监听地址通过标准库按文本解析，跨CBOR保持同一表示。
fn listen_address<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<SocketAddr, D::Error> {
    let text = String::deserialize(deserializer)?;
    text.parse().map_err(serde::de::Error::custom)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            title: String::new(),
            listen: ([127, 0, 0, 1], 0).into(),
            assets: None,
            index: "index.html".into(),
            shutdown_ms: 2000,
            inline_body_bytes: 1024 * 1024,
            max_body_bytes: 16 * 1024 * 1024,
            stream_chunk_bytes: 1024 * 1024,
            route_timeout_ms: 30_000,
            body_idle_ms: 30_000,
            replace_grace_ms: 2000,
            public_origins: Vec::new(),
            page_headers: vec![
                "cookie".into(),
                "user-agent".into(),
                "accept-language".into(),
            ],
        }
    }
}

struct Pages(WebEntry, Arc<registry::Registry>);

#[cordis_sdk::interface]
impl Web for Pages {
    fn entry(&self) -> Result<WebEntry> {
        Ok(self.0.clone())
    }
    fn register(&self, registration: WebRegistration) -> Result<u64> {
        self.1.register(registration)
    }
    fn activate(&self, registration: u64) -> Result<()> {
        self.1.activate(registration)
    }
    fn unregister(&self, registration: u64) -> Result<()> {
        self.1.unregister(registration)
    }
}

pub struct Http;

impl Component for Http {
    const NAME: &'static str = "http";
    type Config = Config;

    fn provides() -> Provides {
        Provides::new()
            .with::<dyn Web>()
            .with::<dyn http_api::Http>()
            .with::<dyn http_api::HttpPage>()
    }

    fn inject() -> Inject {
        Inject::new()
            .optional::<dyn Forward>()
            .with::<dyn cordis_sdk::builtin::DataDir>()
            .with::<dyn cordis_sdk::builtin::Runtime>()
    }

    /// ADR-0038：入口使用静态目录内的相对路径，关闭等待具有正截止时间。
    fn resolve(config: &Config) -> Result<()> {
        if config.index.as_os_str().is_empty()
            || config.index.components().any(|part| {
                matches!(
                    part,
                    PathComponent::ParentDir | PathComponent::RootDir | PathComponent::Prefix(_)
                )
            })
        {
            return Err(Error::new(2, "index需要为assets目录内的相对文件名"));
        }
        if config.shutdown_ms == 0 {
            return Err(Error::new(2, "shutdown_ms需要为正整数"));
        }
        routes::validate_config(config)?;
        Ok(())
    }

    /// PR-1–PR-3、PR-6：服务和线程置于同一效应，服务器退出后撤回该效应。
    async fn apply(ctx: Context, config: Config) -> Result<()> {
        let (finished, completed) = oneshot::channel();
        let effect = ctx.effect("HTTP server", move |scope| start(scope, config, finished))?;
        effect.settled().await?;
        ctx.spawn(async move {
            completed
                .await
                .map_err(|_| Error::new(7, "HTTP线程完成通知已关闭"))?;
            effect.dispose().await
        })
    }
}

/// ADR-0038：先登记通知与join，再启动服务器；启动错误经回执交回组件。
fn start(scope: EffectScope, mut config: Config, finished: oneshot::Sender<()>) -> Result<()> {
    let (address, binding) = listen::restore(scope.context(), config.listen)?;
    config.listen = address;
    let registry = Arc::new(registry::Registry::default());
    scope
        .context()
        .provide::<dyn http_api::HttpPage>(registry.pages.clone())?;
    routes::provide(scope.context(), registry.clone(), &config)?;
    let forward = match scope.context().get::<dyn Forward>() {
        Ok(forward) => Some(socket::Bridge {
            context: scope.context().clone(),
            forward,
        }),
        Err(problem) if problem.code() == 4 => None,
        Err(problem) => return Err(problem),
    };
    let (stop, stopping) = watch::channel(false);
    let worker = Arc::new(Mutex::new(None::<std::thread::JoinHandle<Result<()>>>));
    let retained = worker.clone();
    scope.defer(move || {
        let _closed = stop.send(true);
        let thread = retained.lock().unwrap().take();
        if let Some(thread) = thread {
            thread
                .join()
                .map_err(|_| Error::new(5, "HTTP服务线程发生panic"))??;
        }
        Ok(())
    })?;
    let (ready, started) = std::sync::mpsc::sync_channel(1);
    let pages = registry.clone();
    let context = scope.context().clone();
    let thread = std::thread::Builder::new()
        .name("cordis-http".into())
        .spawn(move || {
            let result = server::run(config, stopping, ready, registry, forward, binding, context);
            let _disposed = finished.send(());
            result
        })
        .map_err(server::problem)?;
    *worker.lock().unwrap() = Some(thread);
    let entry = started
        .recv()
        .map_err(|_| Error::new(7, "HTTP启动通知已关闭"))??;
    scope
        .context()
        .provide::<dyn Web>(Arc::new(Pages(entry, pages)))
}

cordis_sdk::export_plugin!(
    name = "http",
    version = env!("CARGO_PKG_VERSION"),
    components = [Http]
);
