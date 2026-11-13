//! DOC-11 §5、ADR-0042：内存 KV 组件实现，由动态入口与 Rust 调用方共同复用。

use cordis_sdk::prelude::*;
use kv_api::{Changed, Kv, Snapshot};
use std::sync::Mutex;

struct MemoryStore {
    ctx: Context,
    snapshot: Mutex<Snapshot>,
    frozen: Frozen,
}

#[cordis_sdk::interface]
impl Kv for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.snapshot.lock().unwrap().entries.get(key).cloned())
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let mut snapshot = self.snapshot.lock().unwrap();
        self.frozen.check()?;
        snapshot.entries.insert(key.to_owned(), value);
        drop(snapshot);
        self.ctx.emit(Changed {
            key: key.to_owned(),
        })
    }

    fn keys(&self) -> Result<Vec<String>> {
        Ok(self
            .snapshot
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

pub struct KvStore;

impl Component for KvStore {
    const NAME: &'static str = "kv-store";
    type Config = EmptyConfig;

    fn provides() -> Provides {
        Provides::new().with::<dyn Kv>()
    }

    fn state() -> StateSchema {
        StateSchema::new("kv.snapshot", 1, 1)
    }

    /// DOC-11 §5.3：继承快照并登记冻结导出器，服务随上下文效应清理。
    async fn apply(ctx: Context, _config: EmptyConfig) -> Result<()> {
        let snapshot = match ctx.inherited_state()? {
            Some(blob) if blob.schema() == "kv.snapshot" && blob.version() == 1 => {
                blob.decode::<Snapshot>()?
            }
            Some(_) => return Err(Error::new(2, "kv-store 需要 kv.snapshot 版本 1")),
            None => Snapshot::default(),
        };
        let frozen = Frozen::new();
        let store = Arc::new(MemoryStore {
            ctx: ctx.clone(),
            snapshot: Mutex::new(snapshot),
            frozen: frozen.clone(),
        });
        ctx.export_state_with("kv.snapshot", 1, frozen, {
            let store = store.clone();
            move || Ok(store.snapshot.lock().unwrap().clone())
        })?;
        ctx.provide::<dyn Kv>(store)
    }
}
