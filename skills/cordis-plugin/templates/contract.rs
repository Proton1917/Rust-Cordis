//! DOC-11 §8、ADR-0017：接口 {{name}} 的值操作与状态往返契约。

use cordis_contract::Suite;
use cordis_sdk::{builtin::Contract, prelude::*};
use service_api::Counter;

pub struct ContractSuite;
impl Component for ContractSuite {
    const NAME: &'static str = "{{name}}-contract";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>().with::<dyn Contract>()
    }

    /// SK-3：阶段零写入业务值，阶段一检查核心迁移后的值。
    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        let suite = Suite::new(&context)?;
        let counter = context.get::<dyn Counter>()?;
        match suite.stage()? {
            0 => {
                counter.set(42)?;
                suite.equal("set.value", &counter.value()?, &42)?;
                counter.set(17)?;
                suite.equal("set.overwrite", &counter.value()?, &17)?;
                suite.state_roundtrip()
            }
            1 => {
                suite.equal("state.restore", &counter.value()?, &17)?;
                suite.finish()
            }
            stage => Err(Error::new(2, format!("契约阶段 {stage} 超出约定"))),
        }
    }
}

cordis_sdk::export_plugin!(
    name = "{{name}}-contract",
    version = env!("CARGO_PKG_VERSION"),
    components = [ContractSuite],
    rlib_feature = "rlib"
);
