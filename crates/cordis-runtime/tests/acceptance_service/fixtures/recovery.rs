//! CP-11：导出内存值，重启后优先继承已保存的状态。

use cordis_sdk::prelude::*;

#[derive(serde::Deserialize)]
pub struct Configuration {
    initial: u64,
}

pub struct Recovery;

impl Component for Recovery {
    const NAME: &'static str = "recovery";
    type Config = Configuration;

    fn inject() -> Inject {
        Inject::new().with::<dyn cordis_sdk::builtin::DataDir>()
    }

    fn state() -> StateSchema {
        StateSchema::new("recovery.value", 1, 1)
    }

    async fn apply(context: Context, configuration: Configuration) -> Result<()> {
        let state = context.inherited_state()?;
        let value = match &state {
            Some(state) => state.decode::<u64>()?,
            None => configuration.initial,
        };
        context.export_state("recovery.value", 1, move || Ok(value))?;
        let observation = format!("recovery.value={value} inherited={}", state.is_some());
        std::fs::write(context.data_dir()?.join("observation.txt"), observation)
            .map_err(|problem| Error::new(1, problem.to_string()))
    }
}

cordis_sdk::export_plugin!(
    name = "recovery",
    version = env!("CARGO_PKG_VERSION"),
    components = [Recovery]
);
