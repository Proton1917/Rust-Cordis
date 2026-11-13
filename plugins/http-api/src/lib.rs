//! ADR-0046：普通 HTTP 路由、请求与响应流通过现有服务对象传递。
#![forbid(unsafe_code)]

use cordis_sdk::{Bytes, Context, Effect, Result, Service};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
pub use stream_api::ByteStream;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RouteSpec {
    pub prefix: String,
    pub methods: Vec<String>,
    pub max_body_bytes: Option<u64>,
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub prefix: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub peer: String,
    pub body: Body,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

#[derive(Clone, Debug)]
pub enum Body {
    Empty,
    Inline(Bytes),
    Stream {
        length: Option<u64>,
        source: Service<dyn ByteStream>,
    },
}

/// ADR-0039/0046：WIT 变体使用单键映射，空分支的载荷为 null。
impl Serialize for Body {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Empty => serializer.serialize_newtype_variant("Body", 0, "empty", &()),
            Self::Inline(bytes) => serializer.serialize_newtype_variant("Body", 1, "inline", bytes),
            Self::Stream { length, source } => {
                #[derive(Serialize)]
                struct Stream<'a> {
                    length: Option<u64>,
                    source: &'a Service<dyn ByteStream>,
                }
                serializer.serialize_newtype_variant(
                    "Body",
                    2,
                    "stream",
                    &Stream {
                        length: *length,
                        source,
                    },
                )
            }
        }
    }
}

impl<'de> Deserialize<'de> for Body {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum Wire {
            Empty(()),
            Inline(Bytes),
            Stream {
                length: Option<u64>,
                source: Service<dyn ByteStream>,
            },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Empty(()) => Self::Empty,
            Wire::Inline(bytes) => Self::Inline(bytes),
            Wire::Stream { length, source } => Self::Stream { length, source },
        })
    }
}

#[cordis_sdk::interface(name = "cordis.http-handler", version = "1.0")]
pub trait HttpHandler: Send + Sync {
    async fn handle(&self, request: HttpRequest) -> Result<HttpResponse>;
}

#[cordis_sdk::interface(name = "cordis.http", version = "1.0")]
pub trait Http: Send + Sync {
    async fn route(&self, spec: RouteSpec, handler: Service<dyn HttpHandler>) -> Result<u64>;
    async fn unroute(&self, id: u64) -> Result<()>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageAttributes {
    pub headers: Vec<(String, String)>,
    pub peer: String,
}

#[cordis_sdk::interface(name = "cordis.http-page", version = "1.0")]
pub trait HttpPage: Send + Sync {
    async fn attributes(&self, key: Vec<u8>) -> Result<Option<PageAttributes>>;
}

/// ADR-0046 §3.9：来源由核心提供，属性向当前域的 HTTP 提供方查询。
pub async fn page_attributes(
    context: &Context,
    metadata: &cordis_sdk::CborValue,
) -> Result<Option<PageAttributes>> {
    #[derive(Deserialize)]
    struct Metadata {
        #[serde(rename = "cordis.page")]
        page: Option<Source>,
    }
    #[derive(Deserialize)]
    struct Source {
        key: Vec<u8>,
    }
    let metadata: Metadata = cordis_sdk::decode(&cordis_sdk::encode(metadata)?)?;
    match metadata.page {
        Some(page) => context.get::<dyn HttpPage>()?.attributes(page.key).await,
        None => Ok(None),
    }
}

/// ADR-0046 §3.5：登记与撤回归属处理方效应，登记完成后才向调用方返回。
pub async fn register(
    context: &Context,
    spec: RouteSpec,
    handler: Service<dyn HttpHandler>,
) -> Result<Effect> {
    let http = context.get::<dyn Http>()?;
    let effect = context.effect_async("HTTP route", move |scope| async move {
        let id = http.route(spec, handler).await?;
        let retained = http.clone();
        if let Err(problem) = scope.defer_async(move || async move { retained.unroute(id).await }) {
            http.unroute(id).await?;
            return Err(problem);
        }
        Ok(())
    })?;
    effect.settled().await?;
    Ok(effect)
}
