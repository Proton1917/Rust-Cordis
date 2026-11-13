//! ADR-0046 AU-19/20：回显请求，文件读写使用通用字节流与实例数据目录。
#![forbid(unsafe_code)]

use cordis_sdk::{builtin::DataDir, prelude::*};
use http_api::{Body, Http, HttpHandler, HttpRequest, HttpResponse, RouteSpec};
use serde::Deserialize;
use std::{fs, path::PathBuf};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub prefix: String,
    pub label: String,
    pub methods: Vec<String>,
    pub max_body_bytes: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub fail_apply: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            prefix: "/api".into(),
            label: "rust".into(),
            methods: vec!["GET".into(), "POST".into(), "PUT".into(), "DELETE".into()],
            max_body_bytes: None,
            timeout_ms: None,
            fail_apply: false,
        }
    }
}

struct Handler {
    context: Context,
    file: PathBuf,
    label: String,
}

fn error(problem: impl std::fmt::Display) -> Error {
    Error::new(1, problem.to_string())
}

#[cordis_sdk::interface]
impl HttpHandler for Handler {
    async fn handle(&self, request: HttpRequest) -> Result<HttpResponse> {
        let path = request
            .path
            .strip_prefix(&request.prefix)
            .unwrap_or(&request.path);
        match (request.method.as_str(), path) {
            ("PUT" | "POST", "/file") => {
                let size = self.store(request.body).await?;
                json(serde_json::json!({"stored": size, "label": self.label}))
            }
            ("GET", "/file") => Ok(HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "application/octet-stream".into())],
                body: Body::Stream {
                    length: Some(fs::metadata(&self.file).map_err(error)?.len()),
                    source: stream_api::from_file(&self.context, &self.file)?,
                },
            }),
            ("DELETE", "/file") => {
                fs::remove_file(&self.file).map_err(error)?;
                json(serde_json::json!({"removed": true}))
            }
            (_, "/error") => Err(Error::new(1, "HTTP 示例处理错误")),
            (_, "/panic") => panic!("HTTP 示例 panic"),
            (_, "/wait") => {
                self.context.log(3, "HTTP waiting request entered")?;
                self.context
                    .sleep(std::time::Duration::from_secs(60))
                    .await?;
                json(serde_json::json!({"waited": true}))
            }
            (_, "/invalid-status") => Ok(HttpResponse {
                status: 700,
                headers: Vec::new(),
                body: Body::Empty,
            }),
            (_, "/version") => {
                json(serde_json::json!({"label": self.label, "version": env!("CARGO_PKG_VERSION")}))
            }
            _ => self.echo(request).await,
        }
    }
}

impl Handler {
    async fn store(&self, body: Body) -> Result<u64> {
        match body {
            Body::Empty => {
                fs::write(&self.file, []).map_err(error)?;
                Ok(0)
            }
            Body::Inline(bytes) => {
                fs::write(&self.file, &bytes).map_err(error)?;
                Ok(bytes.len() as u64)
            }
            Body::Stream { source, .. } => {
                stream_api::to_file(source, &self.file, 1024 * 1024).await
            }
        }
    }

    async fn echo(&self, request: HttpRequest) -> Result<HttpResponse> {
        let bytes = match request.body {
            Body::Empty => Vec::new(),
            Body::Inline(bytes) => bytes.0,
            Body::Stream { source, .. } => {
                let mut bytes = Vec::new();
                stream_api::copy_to(source, &mut bytes, 1024 * 1024).await?;
                bytes
            }
        };
        let mut response = json(
            serde_json::json!({"method": request.method, "path": request.path, "prefix": request.prefix,
            "query": request.query, "headers": request.headers, "peer": request.peer, "body": bytes, "label": self.label}),
        )?;
        response.headers.extend([
            ("set-cookie".into(), "session=one; HttpOnly; Secure".into()),
            ("set-cookie".into(), "locale=zh; Secure".into()),
            ("connection".into(), "keep-alive".into()),
        ]);
        Ok(response)
    }
}

fn json(value: serde_json::Value) -> Result<HttpResponse> {
    Ok(HttpResponse {
        status: 200,
        headers: vec![("content-type".into(), "application/json".into())],
        body: Body::Inline(serde_json::to_vec(&value).map_err(error)?.into()),
    })
}

struct Echo;
impl Component for Echo {
    const NAME: &'static str = "http-echo";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new().with::<dyn Http>().with::<dyn DataDir>()
    }

    /// ADR-0046 §3.5：路由在激活期间登记，卸下时沿效应自动撤回。
    async fn apply(context: Context, config: Config) -> Result<()> {
        if config.fail_apply {
            return Err(Error::new(1, "HTTP 示例拒绝激活"));
        }
        let directory = PathBuf::from(context.get::<dyn DataDir>()?.path()?);
        fs::create_dir_all(&directory).map_err(error)?;
        let handler = context.service::<dyn HttpHandler>(Arc::new(Handler {
            context: context.clone(),
            file: directory.join("upload.bin"),
            label: config.label,
        }))?;
        http_api::register(
            &context,
            RouteSpec {
                prefix: config.prefix,
                methods: config.methods,
                max_body_bytes: config.max_body_bytes,
                timeout_ms: config.timeout_ms,
            },
            handler,
        )
        .await?;
        Ok(())
    }
}

cordis_sdk::export_plugin!(
    name = "http-echo",
    version = env!("CARGO_PKG_VERSION"),
    components = [Echo]
);
