//! DOC-11 §8：kv@1 的真实读写、键顺序、flush 与状态继承契约套件。

use cordis_contract::Suite;
use cordis_sdk::{builtin::Contract, prelude::*};
use kv_api::Kv;

pub struct KvContract;

impl Component for KvContract {
    const NAME: &'static str = "kv-contract";
    type Config = EmptyConfig;

    fn inject() -> Inject {
        Inject::new().with::<dyn Kv>().with::<dyn Contract>()
    }

    /// ADR-0017：第一阶段写入已知值，第二阶段从重新加载的提供方读取原值。
    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        let suite = Suite::new(&context)?;
        let store = context.get::<dyn Kv>()?;
        match suite.stage()? {
            0 => {
                exercise(&suite, &store).await?;
                suite.state_roundtrip()
            }
            1 => {
                suite.equal(
                    "state.values",
                    &store.get("beta")?,
                    &Some(vec![0, 127, 255]),
                )?;
                suite.equal("state.overwrite", &store.get("alpha")?, &Some(vec![2]))?;
                suite.equal(
                    "state.keys",
                    &store.keys()?,
                    &vec!["alpha".to_owned(), "beta".to_owned()],
                )?;
                suite.finish()
            }
            stage => Err(Error::new(2, format!("kv-contract阶段 {stage} 超出约定"))),
        }
    }
}

/// kv@1：缺席返回 None，字节完整保存，覆盖替换旧值，键按字典顺序返回。
async fn exercise(suite: &Suite, store: &Service<dyn Kv>) -> Result<()> {
    suite.equal("get.missing", &store.get("missing")?, &None::<Vec<u8>>)?;
    store.put("beta", vec![0, 127, 255])?;
    suite.equal("put.bytes", &store.get("beta")?, &Some(vec![0, 127, 255]))?;
    store.put("alpha", vec![1])?;
    store.put("alpha", vec![2])?;
    suite.equal("put.overwrite", &store.get("alpha")?, &Some(vec![2]))?;
    suite.equal(
        "keys.sorted",
        &store.keys()?,
        &vec!["alpha".to_owned(), "beta".to_owned()],
    )?;
    store.flush().await?;
    suite.equal("flush.read", &store.get("beta")?, &Some(vec![0, 127, 255]))
}

cordis_sdk::export_plugin!(
    name = "kv-contract",
    version = env!("CARGO_PKG_VERSION"),
    components = [KvContract],
    rlib_feature = "rlib"
);
