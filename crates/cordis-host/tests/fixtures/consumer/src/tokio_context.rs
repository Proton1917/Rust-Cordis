//! ADR-0055：组件在宿主执行器上直接使用插件库的 tokio 运行时。

use cordis_sdk::prelude::*;
use std::time::Duration;

/// apply 中使用全局 `tokio::spawn` 与 `tokio::time::sleep`；缺少运行时上下文时 apply 失败。
pub struct TokioProbe;

impl Component for TokioProbe {
    const NAME: &'static str = "tokio-probe";
    type Config = EmptyConfig;

    async fn apply(_: Context, _: EmptyConfig) -> Result<()> {
        let worker = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            std::thread::current().name() == Some("cordis-plugin-tokio")
        });
        let on_library_runtime = worker
            .await
            .map_err(|error| Error::new(1, error.to_string()))?;
        if on_library_runtime {
            Ok(())
        } else {
            Err(Error::new(1, "任务没有运行在插件库的 tokio 运行时上"))
        }
    }
}
