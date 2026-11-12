use cordis_sdk::prelude::*;
use kv_api::Kv;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, serde::Deserialize)]
pub struct CounterConfig {
    #[serde(default = "default_interval")]
    pub interval_ms: u64,
    #[serde(default = "default_key")]
    pub key: String,
}

fn default_interval() -> u64 {
    100
}
fn default_key() -> String {
    "count".into()
}

pub struct Counter;

impl Component for Counter {
    const NAME: &'static str = "counter";
    type Config = CounterConfig;

    fn inject() -> Inject {
        Inject::new().with::<dyn Kv>()
    }

    fn state() -> StateSchema {
        StateSchema::new("counter.state", 1, 1)
    }

    async fn apply(ctx: Context, config: CounterConfig) -> Result<()> {
        let kv = ctx.get::<dyn Kv>()?;
        let inherited = match ctx.inherited_state()? {
            Some(blob) if blob.schema() == "counter.state" && blob.version() == 1 => {
                blob.decode::<u64>()?
            }
            Some(_) => return Err(Error::new(2, "counter 需要 counter.state 版本 1")),
            None => 0,
        };
        let stored = kv
            .get(&config.key)?
            .map(|bytes| {
                let value: [u8; 8] = bytes
                    .try_into()
                    .map_err(|_| Error::new(2, "counter 的存储值需要 8 字节"))?;
                Ok::<u64, Error>(u64::from_le_bytes(value))
            })
            .transpose()?
            .unwrap_or(0);
        let count = Arc::new(Mutex::new(inherited.max(stored)));
        let frozen = ctx.export_state("counter.state", 1, {
            let count = count.clone();
            move || Ok(*count.lock().unwrap())
        })?;
        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                while ctx.is_current()? {
                    {
                        let mut value = count.lock().unwrap();
                        if !frozen.is_set() {
                            let next = value
                                .checked_add(1)
                                .ok_or_else(|| Error::new(1, "counter 已达到 u64 上限"))?;
                            match kv.put(&config.key, next.to_le_bytes().to_vec()) {
                                Ok(()) => *value = next,
                                Err(error) if error.code() == 4 => {}
                                Err(error) => return Err(error),
                            }
                        }
                    }
                    ctx.sleep(Duration::from_millis(config.interval_ms)).await?;
                }
                Ok(())
            }
        })
    }
}

cordis_sdk::export_plugin!(
    name = "counter",
    version = env!("CARGO_PKG_VERSION"),
    components = [Counter],
    rlib_feature = "rlib"
);
