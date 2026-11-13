//! ADR-0046 AU-23：真实登记检查冲突，独立子前缀继续提供可观察响应。
use cordis_sdk::prelude::*;
use http_api::{Body, Http, HttpHandler, HttpRequest, HttpResponse, RouteSpec};

struct Handler;
#[cordis_sdk::interface]
impl HttpHandler for Handler {
    async fn handle(&self, _: HttpRequest) -> Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: Body::Inline(b"nested-route".to_vec().into()),
        })
    }
}

#[derive(serde::Deserialize)]
pub struct Config {
    prefixes: Vec<String>,
    result: String,
}
pub struct Probe;
impl Component for Probe {
    const NAME: &'static str = "route-conflicts";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new().with::<dyn Http>()
    }
    async fn apply(context: Context, config: Config) -> Result<()> {
        let http = context.get::<dyn Http>()?;
        let handler = context.service::<dyn HttpHandler>(Arc::new(Handler))?;
        let mut observations = Vec::new();
        for prefix in config.prefixes {
            let result = http.route(spec(prefix.clone()), handler.clone()).await;
            let code = match result {
                Err(problem) => problem.code(),
                Ok(id) => {
                    http.unroute(id).await?;
                    return Err(Error::new(1, "冲突路由被接受"));
                }
            };
            if code != 2 {
                return Err(Error::new(1, "路由冲突错误码失配"));
            }
            observations.push(serde_json::json!({"prefix": prefix, "code": code}));
        }
        http_api::register(&context, spec("/api/nested".into()), handler).await?;
        std::fs::write(
            config.result,
            serde_json::to_vec(&observations).map_err(problem)?,
        )
        .map_err(problem)
    }
}

fn spec(prefix: String) -> RouteSpec {
    RouteSpec {
        prefix,
        methods: vec!["GET".into()],
        max_body_bytes: None,
        timeout_ms: None,
    }
}
fn problem(problem: impl std::fmt::Display) -> Error {
    Error::new(1, problem.to_string())
}
