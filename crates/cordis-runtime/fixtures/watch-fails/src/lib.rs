//! ADR-0031：实际替换 watch 的失败库，观察原任务清理后的窗口恢复。

use cordis_sdk::prelude::*;

struct Watch;

impl Component for Watch {
    const NAME: &'static str = "watch";
    type Config = EmptyConfig;

    async fn apply(_: Context, _: EmptyConfig) -> Result<()> {
        Err(Error::new(1002, "watch候选加载失败"))
    }
}

cordis_sdk::export_plugin!(
    name = "watch",
    version = env!("CARGO_PKG_VERSION"),
    components = [Watch]
);
