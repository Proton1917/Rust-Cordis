//! ADR-0047 AU-27a：真实文件流的最后引用归还后记录释放，类型化解码保留可读资源。

use super::problem;
use cordis_sdk::prelude::*;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use stream_api::ByteStream;

#[cordis_sdk::interface(name = "dynamic.streams", version = "1.0")]
trait Streams: Send + Sync {
    fn open(&self) -> Result<Service<dyn ByteStream>>;
    fn released(&self) -> Result<u64>;
}

struct Released(Arc<AtomicU64>);
impl Drop for Released {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Source {
    source: Service<dyn ByteStream>,
    _released: Released,
}
#[cordis_sdk::interface]
impl ByteStream for Source {
    async fn read(&self, max_bytes: u32) -> Result<Option<Vec<u8>>> {
        self.source.read(max_bytes).await
    }
}

struct Factory {
    context: Context,
    file: String,
    released: Arc<AtomicU64>,
}
#[cordis_sdk::interface]
impl Streams for Factory {
    fn open(&self) -> Result<Service<dyn ByteStream>> {
        self.context.service::<dyn ByteStream>(Arc::new(Source {
            source: stream_api::from_file(&self.context, &self.file)?,
            _released: Released(self.released.clone()),
        }))
    }
    fn released(&self) -> Result<u64> {
        Ok(self.released.load(Ordering::SeqCst))
    }
}

#[derive(serde::Deserialize)]
pub struct Config {
    file: String,
}
pub struct Provider;
impl Component for Provider {
    const NAME: &'static str = "streams";
    type Config = Config;
    fn provides() -> Provides {
        Provides::new().with::<dyn Streams>()
    }
    async fn apply(context: Context, config: Config) -> Result<()> {
        context.provide::<dyn Streams>(Arc::new(Factory {
            context: context.clone(),
            file: config.file,
            released: Arc::new(AtomicU64::new(0)),
        }))
    }
}

async fn released(context: &Context, provider: &DynamicService, expected: u64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while provider.invoke_as::<_, u64>("released", &Vec::<CborValue>::new())? < expected {
        if Instant::now() >= deadline {
            return Err(Error::new(1, "字节流资源释放超时"));
        }
        context.sleep(Duration::from_millis(10)).await?;
    }
    Ok(())
}

pub async fn exercise(context: &Context) -> Result<serde_json::Value> {
    let provider = context.get_dynamic("dynamic.streams@1.0")?;
    let before: u64 = provider.invoke_as("released", &Vec::<CborValue>::new())?;
    let rejected = provider
        .invoke("open", CborValue::Array(Vec::new()))
        .err()
        .ok_or_else(|| Error::new(1, "原始资源结果取得成功"))?;
    if rejected.code() != 2 {
        return Err(rejected);
    }
    released(context, &provider, before + 1).await?;
    let stream: Service<dyn ByteStream> = provider
        .invoke_as_async("open", &Vec::<CborValue>::new())
        .await?;
    let mut count = 0usize;
    while let Some(chunk) = stream.read(64 * 1024).await? {
        for byte in chunk {
            if byte != (count % 256) as u8 {
                return Err(Error::new(1, "文件流内容失配"));
            }
            count += 1;
        }
    }
    if count != 1024 * 1024 {
        return Err(problem(format!("字节流长度为 {count}")));
    }
    drop(stream);
    released(context, &provider, before + 2).await?;
    Ok(
        serde_json::json!({"raw_result_code":2,"raw_result_released":true,"bytes":count,"typed_resource_released":true}),
    )
}
