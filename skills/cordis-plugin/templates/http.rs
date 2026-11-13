use cordis_sdk::prelude::*;
use http_api::{Body, Http, HttpHandler, HttpRequest, HttpResponse, RouteSpec};

struct Handler;
#[cordis_sdk::interface]
impl HttpHandler for Handler {
    async fn handle(&self, request: HttpRequest) -> Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())],
            body: Body::Inline(
                format!("{} {}\n", request.method, request.path)
                    .into_bytes()
                    .into(),
            ),
        })
    }
}

struct Plugin;
impl Component for Plugin {
    const NAME: &'static str = "{{name}}";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Http>()
    }
    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        let handler = context.service::<dyn HttpHandler>(Arc::new(Handler))?;
        http_api::register(
            &context,
            RouteSpec {
                prefix: "/{{name}}".into(),
                methods: Vec::new(),
                max_body_bytes: None,
                timeout_ms: None,
            },
            handler,
        )
        .await?;
        Ok(())
    }
}

cordis_sdk::export_plugin!(
    name = "{{name}}",
    version = env!("CARGO_PKG_VERSION"),
    components = [Plugin]
);
