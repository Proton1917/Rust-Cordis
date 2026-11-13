//! ADR-0046 AU-24：同组和跨组使用方以相同字节流接口读文件，并显式释放中途停止的流。

use cordis_sdk::prelude::*;
use stream_api::ByteStream;

#[cordis_sdk::interface(name = "test.stream-files", version = "1.0")]
trait Files: Send + Sync {
    fn open(&self) -> Result<Service<dyn ByteStream>>;
}

#[derive(serde::Deserialize)]
pub struct ProviderConfig {
    file: String,
}
struct Source {
    context: Context,
    file: String,
}
#[cordis_sdk::interface]
impl Files for Source {
    fn open(&self) -> Result<Service<dyn ByteStream>> {
        stream_api::from_file(&self.context, &self.file)
    }
}
pub struct Provider;
impl Component for Provider {
    const NAME: &'static str = "stream-provider";
    type Config = ProviderConfig;
    fn provides() -> Provides {
        Provides::new().with::<dyn Files>()
    }
    async fn apply(context: Context, config: ProviderConfig) -> Result<()> {
        context.provide::<dyn Files>(Arc::new(Source {
            context: context.clone(),
            file: config.file,
        }))
    }
}

#[derive(serde::Deserialize)]
pub struct Config {
    output: String,
    result: String,
    cancel: bool,
    typed: bool,
}
pub struct Probe;
impl Component for Probe {
    const NAME: &'static str = "stream-probe";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new().with::<dyn Files>()
    }
    async fn apply(context: Context, config: Config) -> Result<()> {
        let source = context.get::<dyn Files>()?.open()?;
        let typed = source.clone_object()?.has_typed_methods();
        if typed != config.typed {
            return Err(Error::new(1, "字节流实际插口类型错误"));
        }
        let count = if config.cancel {
            let bytes = source
                .read(1024 * 1024)
                .await?
                .ok_or_else(|| Error::new(1, "文件流提前结束"))?;
            if bytes.len() > 1024 * 1024 {
                return Err(Error::new(1, "流块超过请求长度"));
            }
            drop(source);
            bytes.len() as u64
        } else {
            stream_api::to_file(source, config.output, 1024 * 1024).await?
        };
        let result = serde_json::json!({"bytes": count, "source_typed": typed, "cancelled": config.cancel, "chunk_limit": 1048576});
        std::fs::write(config.result, serde_json::to_vec(&result).map_err(problem)?)
            .map_err(problem)
    }
}

fn problem(problem: impl std::fmt::Display) -> Error {
    Error::new(1, problem.to_string())
}
