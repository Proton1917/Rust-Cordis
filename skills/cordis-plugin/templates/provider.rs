//! DOC-11 §5：接口 {{name}} 的状态提供方。

use cordis_sdk::prelude::*;
use service_api::{Changed, Counter, Snapshot};
use std::sync::Mutex;

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    pub initial: u64,
}

struct Store {
    context: Context,
    state: Mutex<Snapshot>,
    frozen: Frozen,
}

#[cordis_sdk::interface]
impl Counter for Store {
    fn value(&self) -> Result<u64> {
        Ok(self.state.lock().unwrap().value)
    }

    fn set(&self, value: u64) -> Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            self.frozen.check()?;
            state.value = value;
        }
        self.context.emit(Changed { value })
    }
}

pub struct Provider;
impl Component for Provider {
    const NAME: &'static str = "{{name}}";
    type Config = Config;
    fn provides() -> Provides {
        Provides::new().with::<dyn Counter>()
    }

    fn state() -> StateSchema {
        StateSchema::new("{{name}}.snapshot", 1, 1)
    }

    /// SW-5：同一把锁保护检查、修改与快照，事件在释放状态锁后派发。
    async fn apply(context: Context, config: Config) -> Result<()> {
        let snapshot = match context.inherited_state()? {
            Some(state) if state.schema() == "{{name}}.snapshot" && state.version() == 1 => {
                state.decode::<Snapshot>()?
            }
            Some(_) => return Err(Error::new(2, "状态模式或版本与提供方声明不符")),
            None => Snapshot { value: config.initial },
        };
        let frozen = Frozen::new();
        let store = Arc::new(Store {
            context: context.clone(),
            state: Mutex::new(snapshot),
            frozen: frozen.clone(),
        });
        let exported = store.clone();
        context.export_state_with("{{name}}.snapshot", 1, frozen, move || {
            Ok(exported.state.lock().unwrap().clone())
        })?;
        context.provide::<dyn Counter>(store)
    }
}

cordis_sdk::export_plugin!(
    name = "{{name}}",
    version = env!("CARGO_PKG_VERSION"),
    components = [Provider],
    rlib_feature = "rlib"
);
