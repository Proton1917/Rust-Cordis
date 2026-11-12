//! DOC-14 AC-2–AC-4：同一逻辑 KV 的新版本、加载失败版本与独立主版本。

use cordis_sdk::prelude::*;

#[cfg(feature = "incompatible")]
mod next_interface {
    use super::*;
    use kv_api::Snapshot;
    use std::sync::Mutex;

    #[cordis_sdk::interface(name = "kv", version = "2.0")]
    pub trait Kv: Send + Sync {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
        fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;
        fn keys(&self) -> Result<Vec<String>>;
        async fn flush(&self) -> Result<()>;
    }

    struct Store {
        entries: Mutex<Snapshot>,
        frozen: Frozen,
    }

    #[cordis_sdk::interface]
    impl Kv for Store {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.entries.lock().unwrap().entries.get(key).cloned())
        }
        fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
            let mut entries = self.entries.lock().unwrap();
            self.frozen.check()?;
            entries.entries.insert(key.to_owned(), value);
            Ok(())
        }
        fn keys(&self) -> Result<Vec<String>> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .entries
                .keys()
                .cloned()
                .collect())
        }
        async fn flush(&self) -> Result<()> {
            Ok(())
        }
    }

    /// DOC-11 §3/§5：主版本 2 有独立服务身份，状态格式继续采用 kv.snapshot@1。
    pub async fn apply(context: Context) -> Result<()> {
        let snapshot = match context.inherited_state()? {
            Some(blob) => blob.decode::<Snapshot>()?,
            None => Snapshot::default(),
        };
        let frozen = Frozen::new();
        let store = Arc::new(Store {
            entries: Mutex::new(snapshot),
            frozen: frozen.clone(),
        });
        let exported = store.clone();
        context.export_state_with("kv.snapshot", 1, frozen, move || {
            Ok(exported.entries.lock().unwrap().clone())
        })?;
        context.provide::<dyn Kv>(store)
    }
}

struct Store;

impl Component for Store {
    const NAME: &'static str = "kv-store";
    type Config = EmptyConfig;

    fn provides() -> Provides {
        #[cfg(feature = "incompatible")]
        {
            Provides::new().with::<dyn next_interface::Kv>()
        }
        #[cfg(not(feature = "incompatible"))]
        {
            kv_mem::KvStore::provides()
        }
    }

    fn state() -> StateSchema {
        kv_mem::KvStore::state()
    }

    /// DOC-14 AC-3：失败版本在实际登记服务后返回错误，验证完整效应回滚。
    async fn apply(context: Context, config: EmptyConfig) -> Result<()> {
        #[cfg(feature = "incompatible")]
        {
            let _config = config;
            next_interface::apply(context).await?;
        }
        #[cfg(not(feature = "incompatible"))]
        kv_mem::KvStore::apply(context, config).await?;
        if cfg!(feature = "fails") {
            return Err(Error::new(1002, "KV新版本加载失败"));
        }
        Ok(())
    }
}

cordis_sdk::export_plugin!(
    name = "kv",
    version = env!("CARGO_PKG_VERSION"),
    components = [Store]
);
