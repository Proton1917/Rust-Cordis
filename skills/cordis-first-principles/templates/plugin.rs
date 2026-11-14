//! DOC-15 §12.1：由实例配置读取消息，在组件激活时记录日志。

use cordis_sdk::prelude::*;
use std::collections::BTreeMap;

pub struct Plugin;

impl Component for Plugin {
    const NAME: &'static str = "{{name}}";
    type Config = BTreeMap<String, String>;

    /// PR-1：日志归属当前实例，宿主错误沿 apply 返回。
    async fn apply(context: Context, config: Self::Config) -> Result<()> {
        let message = config
            .get("message")
            .map(String::as_str)
            .unwrap_or("{{name}} 已激活");
        context.log(2, message)
    }
}

cordis_sdk::export_plugin!(
    name = "{{name}}",
    version = env!("CARGO_PKG_VERSION"),
    components = [Plugin]
);
