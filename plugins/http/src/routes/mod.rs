//! ADR-0046 §3.2/3.5：HTTP 前缀登记与网页资源共享锁，替换期间按通知等待新处理方。

mod body;
mod headers;
mod request;

use super::{Config, registry::Registry};
use axum::http::Method;
use cordis_sdk::{CborValue, Context, Error, Result, Scoped, Service as Remote};
use http_api::{Http, HttpHandler, RouteSpec};
use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

pub(super) struct Route {
    pub id: u64,
    pub spec: RouteSpec,
    pub handler: Remote<dyn HttpHandler>,
    pub owner: Option<u64>,
}

pub(super) enum Slot {
    Active(Arc<Route>),
    Retired { deadline: Instant, failed: bool },
}

struct Service {
    pub registry: Arc<Registry>,
    pub config: Config,
    pub owner: Option<u64>,
}

/// ADR-0046 §3.5：每个 Scoped 使用方持有自己的登记所有者，视图释放时撤回其路由。
pub(super) fn provide(context: &Context, registry: Arc<Registry>, config: &Config) -> Result<()> {
    let base: Arc<dyn Http> = Arc::new(Service {
        registry: registry.clone(),
        config: config.clone(),
        owner: None,
    });
    let config = config.clone();
    context.provide_scoped::<dyn Http>(Scoped::new(base, move |_: CborValue| {
        let owner = registry.next.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(Arc::new(Service {
            registry: registry.clone(),
            config: config.clone(),
            owner: Some(owner),
        }) as Arc<dyn Http>)
    }))
}

impl Drop for Service {
    /// ADR-0046 §3.5：使用方视图退役时归还 scoped 对象，断连后的登记随该对象撤回。
    fn drop(&mut self) {
        let Some(owner) = self.owner else {
            return;
        };
        let ids: Vec<_> = self
            .registry
            .state
            .lock()
            .unwrap()
            .routes
            .values()
            .filter_map(|slot| match slot {
                Slot::Active(route) if route.owner == Some(owner) => Some(route.id),
                _ => None,
            })
            .collect();
        for id in ids {
            retire(&self.registry, id, self.config.replace_grace_ms, true);
        }
    }
}

#[cordis_sdk::interface]
impl Http for Service {
    async fn route(&self, spec: RouteSpec, handler: Remote<dyn HttpHandler>) -> Result<u64> {
        validate_spec(&spec, &self.config)?;
        let mut state = self.registry.state.lock().unwrap();
        if matches!(state.routes.get(&spec.prefix), Some(Slot::Active(_)))
            || state
                .versions
                .values()
                .any(|entry| overlaps(&entry.info.mount, &spec.prefix))
        {
            return Err(Error::new(2, "HTTP 路由前缀已登记或与网页挂载重叠"));
        }
        let id = self.registry.next.fetch_add(1, Ordering::Relaxed) + 1;
        let prefix = spec.prefix.clone();
        let discarded = state.routes.insert(
            prefix,
            Slot::Active(Arc::new(Route {
                id,
                spec,
                handler,
                owner: self.owner,
            })),
        );
        drop(state);
        drop(discarded);
        self.registry.changed.send_replace(());
        Ok(id)
    }

    async fn unroute(&self, id: u64) -> Result<()> {
        {
            let state = self.registry.state.lock().unwrap();
            if state.routes.values().any(|slot| matches!(slot, Slot::Active(route) if route.id == id && route.owner != self.owner)) {
                return Err(Error::new(9, "路由撤回需要原登记视图"));
            }
        }
        retire(&self.registry, id, self.config.replace_grace_ms, false);
        Ok(())
    }
}

/// ADR-0046 §3.5：精确撤回登记编号，旧版本清理保持新版本的同名前缀。
fn retire(registry: &Registry, id: u64, grace_ms: u64, failed: bool) {
    let discarded = {
        let mut state = registry.state.lock().unwrap();
        let prefix = state.routes.iter().find_map(|(prefix, slot)| match slot {
            Slot::Active(route) if route.id == id => Some(prefix.clone()),
            _ => None,
        });
        prefix.and_then(|prefix| {
            state.routes.insert(
                prefix,
                Slot::Retired {
                    deadline: Instant::now() + Duration::from_millis(grace_ms),
                    failed,
                },
            )
        })
    };
    drop(discarded);
    registry.changed.send_replace(());
}

pub(super) fn matches(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|tail| tail.starts_with('/'))
}

pub(super) fn overlaps(left: &str, right: &str) -> bool {
    left == "/"
        || right == "/"
        || matches(left.trim_end_matches('/'), right.trim_end_matches('/'))
        || matches(right.trim_end_matches('/'), left.trim_end_matches('/'))
}

fn validate_spec(spec: &RouteSpec, config: &Config) -> Result<()> {
    let uri = spec
        .prefix
        .parse::<axum::http::Uri>()
        .map_err(super::server::problem)?;
    if !spec.prefix.starts_with('/')
        || spec.prefix.starts_with("/_cordis")
        || uri.path() != spec.prefix
        || (spec.prefix.len() > 1 && spec.prefix.ends_with('/'))
    {
        return Err(Error::new(2, "HTTP 路由需要规范的绝对路径段前缀"));
    }
    for method in &spec.methods {
        Method::from_bytes(method.as_bytes()).map_err(super::server::problem)?;
    }
    if spec
        .max_body_bytes
        .is_some_and(|size| size > config.max_body_bytes)
        || spec.timeout_ms == Some(0)
    {
        return Err(Error::new(
            2,
            "路由上限需要处于实例预算内，截止时间需要为正数",
        ));
    }
    Ok(())
}

pub(super) fn validate_config(config: &Config) -> Result<()> {
    if config.inline_body_bytes > u64::from(stream_api::MAX_CHUNK_BYTES)
        || config.stream_chunk_bytes == 0
        || config.stream_chunk_bytes > stream_api::MAX_CHUNK_BYTES
        || config.route_timeout_ms == 0
        || config.body_idle_ms == 0
    {
        return Err(Error::new(
            2,
            "HTTP 块长需要处于 4 MiB 内，截止时间需要为正数",
        ));
    }
    for origin in &config.public_origins {
        let parsed = url::Url::parse(origin).map_err(super::server::problem)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.origin().ascii_serialization() != *origin
        {
            return Err(Error::new(2, "公开来源需要完整的 HTTP 或 HTTPS origin"));
        }
    }
    for header in &config.page_headers {
        axum::http::HeaderName::from_bytes(header.as_bytes()).map_err(super::server::problem)?;
    }
    Ok(())
}

pub(super) use request::{State, dispatch};
