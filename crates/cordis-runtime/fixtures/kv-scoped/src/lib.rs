//! DOC-15 AU-18：真实 KV 视图记录收到的元数据，键值操作保持 kv@1 契约。

use cordis_sdk::{CborValue, builtin::DataDir, prelude::*};
use kv_api::Kv;
use std::{collections::BTreeMap, sync::Mutex};

struct Memory(Mutex<BTreeMap<String, Vec<u8>>>);

#[cordis_sdk::interface]
impl Kv for Memory {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), value);
        Ok(())
    }
    fn keys(&self) -> Result<Vec<String>> {
        Ok(self.0.lock().unwrap().keys().cloned().collect())
    }
    async fn flush(&self) -> Result<()> {
        Ok(())
    }
}

struct Store;
impl Component for Store {
    const NAME: &'static str = "store";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn DataDir>()
    }
    fn provides() -> Provides {
        Provides::new().with::<dyn Kv>()
    }

    /// ADR-0025：scope工厂保存实际元数据并交出同一真实数据的接口对象。
    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        let observed = context.data_dir()?.join("metadata.cbor");
        let store = Arc::new(Memory(Mutex::new(BTreeMap::new())));
        let retained = store.clone();
        context.provide_scoped::<dyn Kv>(Scoped::new(store, move |metadata: CborValue| {
            std::fs::write(&observed, cordis_sdk::encode(&metadata)?)
                .map_err(|error| Error::new(1, error.to_string()))?;
            Ok(retained.clone())
        }))
    }
}

#[derive(serde::Serialize)]
struct Observation {
    typed: bool,
    value: Option<Vec<u8>>,
    keys: Vec<String>,
}

struct Client;
impl Component for Client {
    const NAME: &'static str = "client";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Kv>().with::<dyn DataDir>()
    }

    /// AU-18：相同代码分别接收组内直连与跨组/进程代理，实际读写后记录结果。
    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        let service = context.get::<dyn Kv>()?;
        let typed = service.clone_object()?.has_typed_methods();
        service.put("probe", vec![4, 2, 9])?;
        service.flush().await?;
        let result = Observation {
            typed,
            value: service.get("probe")?,
            keys: service.keys()?,
        };
        std::fs::write(
            context.data_dir()?.join("client.cbor"),
            cordis_sdk::encode(&result)?,
        )
        .map_err(|error| Error::new(1, error.to_string()))
    }
}

cordis_sdk::export_plugin!(
    name = "kv-scoped",
    version = env!("CARGO_PKG_VERSION"),
    components = [Store, Client]
);
