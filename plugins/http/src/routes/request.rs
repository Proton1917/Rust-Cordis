//! ADR-0046 §3：普通 HTTP 请求沿动态前缀分派，状态转换在提交响应头前完成。

use super::super::{Config, registry::Registry};
use super::{Route, Slot, body, headers};
use axum::body::HttpBody as _;
use axum::{
    body::Body,
    extract::{ConnectInfo, State as Extract},
    http::{Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use cordis_sdk::Context;
use http_api::{HttpRequest, HttpResponse};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct State {
    pub registry: Arc<Registry>,
    pub context: Context,
    pub config: Config,
}

enum Selection {
    Active(Arc<Route>),
    Waiting(Instant),
    Absent,
    Failed,
}

/// ADR-0046 §3.2：最长段前缀优先，显式撤回的宽限结束后恢复原静态路由。
fn select(registry: &Registry, path: &str) -> Selection {
    let mut state = registry.state.lock().unwrap();
    state.routes.retain(|_, slot| !matches!(slot, Slot::Retired { deadline, failed: false } if *deadline < Instant::now()));
    match state
        .routes
        .iter()
        .filter(|(prefix, _)| super::matches(prefix, path))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, slot)| slot)
    {
        Some(Slot::Active(route)) => Selection::Active(route.clone()),
        Some(Slot::Retired { deadline, .. }) if *deadline > Instant::now() => {
            Selection::Waiting(*deadline)
        }
        Some(Slot::Retired { .. }) => Selection::Failed,
        None => Selection::Absent,
    }
}

async fn resolve(registry: &Registry, path: &str) -> Result<Option<Arc<Route>>, StatusCode> {
    let mut changed = registry.changed.subscribe();
    let mut waiting = false;
    loop {
        match select(registry, path) {
            Selection::Active(route) => return Ok(Some(route)),
            Selection::Absent if !waiting => return Ok(None),
            Selection::Absent | Selection::Failed => return Err(StatusCode::SERVICE_UNAVAILABLE),
            Selection::Waiting(deadline) => {
                waiting = true;
                tokio::time::timeout_at(deadline.into(), changed.changed())
                    .await
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            }
        }
    }
}

pub(crate) async fn dispatch(
    Extract(state): Extract<Arc<State>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.uri().path() == "/_cordis" || request.uri().path().starts_with("/_cordis/") {
        return static_response(next.run(request).await);
    }
    match resolve(&state.registry, request.uri().path()).await {
        Ok(Some(route)) => execute(&state, route, peer, request).await,
        Ok(None) => static_response(next.run(request).await),
        Err(status) => failure(status),
    }
}

/// ADR-0046、默认外壳：静态缺席响应携带中性正文，浏览器可直接显示该状态。
fn static_response(mut response: Response) -> Response {
    if response.status() == StatusCode::NOT_FOUND && response.body().is_end_stream() {
        let text = "Not Found\n";
        *response.body_mut() = Body::from(text);
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, text.len().into());
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            "text/plain; charset=utf-8".parse().expect("固定内容类型"),
        );
    }
    response
}

/// ADR-0046 §3.3–3.5：确认方法、装配请求，再等待处理方的响应或截止时间。
async fn execute(
    state: &State,
    route: Arc<Route>,
    peer: SocketAddr,
    request: Request<Body>,
) -> Response {
    if !route.spec.methods.is_empty()
        && !route
            .spec
            .methods
            .iter()
            .any(|method| method == request.method().as_str())
    {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, route.spec.methods.join(", "))],
        )
            .into_response();
    }
    let (request, guard) = match prepare_request(state, &route, peer, request).await {
        Ok(prepared) => prepared,
        Err(status) => return failure(status),
    };
    invoke(state, &route, request, guard).await
}

/// ADR-0046 §3.3/3.4：边界处转换头部和正文，上传守卫随整个响应处理持有。
async fn prepare_request(
    state: &State,
    route: &Route,
    peer: SocketAddr,
    request: Request<Body>,
) -> Result<(HttpRequest, body::UploadGuard), StatusCode> {
    let (parts, incoming) = request.into_parts();
    let headers = headers::request(&parts.headers)?;
    let length = parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .or_else(|| incoming.size_hint().exact());
    let limit = route
        .spec
        .max_body_bytes
        .unwrap_or(state.config.max_body_bytes);
    let (body, guard) =
        body::prepare(&state.context, incoming, length, &state.config, limit).await?;
    let request = HttpRequest {
        method: parts.method.to_string(),
        path: parts.uri.path().into(),
        prefix: route.spec.prefix.clone(),
        query: parts.uri.query().unwrap_or_default().into(),
        headers,
        peer: peer.to_string(),
        body,
    };
    Ok((request, guard))
}

/// ADR-0046 §3.5：处理错误按状态转换；响应头提交前完成上传额度计量。
async fn invoke(
    state: &State,
    route: &Route,
    request: HttpRequest,
    guard: body::UploadGuard,
) -> Response {
    let timeout = Duration::from_millis(
        route
            .spec
            .timeout_ms
            .unwrap_or(state.config.route_timeout_ms),
    );
    let result = tokio::time::timeout(timeout, route.handler.handle(request)).await;
    if let Some(status) = guard.0.status() {
        return failure(status);
    }
    match result {
        Ok(Ok(response)) => {
            if let Err(status) = body::finish(&guard.0, state.config.stream_chunk_bytes).await {
                return failure(status);
            }
            response_body(response, &state.config).unwrap_or_else(failure)
        }
        Ok(Err(problem)) => {
            if let Err(logging) = state
                .context
                .log(3, &format!("HTTP 路由 {}：{problem}", route.spec.prefix))
            {
                eprintln!("HTTP 路由日志：{logging}");
            }
            if matches!(problem.code(), 4 | 7) {
                super::retire(
                    &state.registry,
                    route.id,
                    state.config.replace_grace_ms,
                    true,
                );
                failure(StatusCode::SERVICE_UNAVAILABLE)
            } else {
                failure(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
        Err(_) => failure(StatusCode::GATEWAY_TIMEOUT),
    }
}

fn response_body(response: HttpResponse, config: &Config) -> Result<Response, StatusCode> {
    if !(100..=599).contains(&response.status) {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let status = StatusCode::from_u16(response.status).map_err(|_| StatusCode::BAD_GATEWAY)?;
    let mut headers = headers::response(response.headers)?;
    let (body, length) = body::response(response.body, config);
    if let Some(length) = length {
        headers.insert(header::CONTENT_LENGTH, length.into());
    }
    Ok((status, headers, body).into_response())
}

fn failure(status: StatusCode) -> Response {
    if status == StatusCode::SERVICE_UNAVAILABLE {
        (
            status,
            [(header::RETRY_AFTER, "1")],
            "HTTP handler unavailable\n",
        )
            .into_response()
    } else {
        (
            status,
            format!(
                "{}\n",
                status.canonical_reason().unwrap_or("HTTP request failed")
            ),
        )
            .into_response()
    }
}
