//! ADR-0055：插件代码在没有 tokio 上下文的线程上被调用时，SDK 提供库级运行时。

#[allow(dead_code)]
#[path = "support/host.rs"]
mod host;

use cordis_abi::testing::LoadedPlugin;
use cordis_sdk::prelude::*;
use std::time::Duration;

#[derive(serde::Serialize, serde::Deserialize)]
struct NetworkConfig {
    delay_ms: u64,
}

/// 组件在 apply 中直接使用全局 `tokio::spawn` 与 `tokio::time::sleep`，不自建运行时。
struct Network;
impl Component for Network {
    const NAME: &'static str = "network";
    type Config = NetworkConfig;
    async fn apply(ctx: Context, config: NetworkConfig) -> Result<()> {
        let worker = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(config.delay_ms)).await;
            std::thread::current().name().map(str::to_owned)
        });
        let thread = worker
            .await
            .map_err(|error| Error::new(1, error.to_string()))?;
        ctx.log(1, &format!("tokio worker {}", thread.unwrap_or_default()))
    }
}
cordis_sdk::export_plugin!(name = "network", version = "1.0.0", components = [Network]);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_code_gets_library_tokio_context_on_plain_threads() -> Result<()> {
    let host = Arc::new(host::Loopback::new());
    cordis_abi::testing::install(host.clone())?;
    let config = cordis_sdk::encode(&NetworkConfig { delay_ms: 5 })?;
    let id = host.context(config.clone(), Vec::new(), None);

    // 普通系统线程没有任何 tokio 上下文；插件代码只能依靠 SDK 登记的库级运行时。
    let caller = std::thread::spawn(move || -> Result<bool> {
        assert!(tokio::runtime::Handle::try_current().is_err());
        let plugin = LoadedPlugin::load(cordis_entry(), 3)?;
        futures_lite::future::block_on(plugin.apply(0, id, &config)?)?;
        let running = cordis_sdk::__private::ambient_running();
        drop(plugin);
        assert!(!cordis_sdk::__private::ambient_running());
        Ok(running)
    });
    let running = tokio::task::spawn_blocking(move || caller.join())
        .await
        .map_err(|error| Error::new(1, error.to_string()))?
        .map_err(|_| Error::new(5, "调用线程 panic"))??;

    assert!(running, "插件运行期间库级运行时应在运行");
    assert!(
        host.logs()
            .iter()
            .any(|line| line == "tokio worker cordis-plugin-tokio"),
        "{:?}",
        host.logs()
    );
    Ok(())
}
