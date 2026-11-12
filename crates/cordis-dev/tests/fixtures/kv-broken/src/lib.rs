//! DOC-11 §8：真实原生库的违约行为与进程退出供验证门识别。

use cordis_sdk::prelude::*;
use kv_api::Kv;

struct BrokenStore;

#[cordis_sdk::interface]
impl Kv for BrokenStore {
    fn get(&self, _key: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn put(&self, _key: &str, _value: Vec<u8>) -> Result<()> {
        Ok(())
    }

    fn keys(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn flush(&self) -> Result<()> {
        Ok(())
    }
}

pub struct Store;

impl Component for Store {
    const NAME: &'static str = "kv-store";
    type Config = EmptyConfig;

    fn provides() -> Provides {
        Provides::new().with::<dyn Kv>()
    }

    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        if cfg!(feature = "abort") {
            std::process::abort();
        }
        context.provide::<dyn Kv>(Arc::new(BrokenStore))
    }
}

cordis_sdk::export_plugin!(
    name = "kv",
    version = env!("CARGO_PKG_VERSION"),
    components = [Store]
);
