//! ADR-0046 §3.10：按插件实际服务声明识别密钥路径，复制阶段跳过这些文件。

use super::{Plan, Result, absolute};
use cordis_up::payload::Plugin;
use std::path::Path;
use toml::{Table, Value};

/// 发布前读取原生描述符，装配共用本轮结果；文件内容始终由密钥提供方读取。
pub(super) fn collect(plan: &mut Plan, descriptor: &Plugin, instances: &[Table]) -> Result<()> {
    for instance in instances {
        let selected = instance.get("component").and_then(Value::as_str);
        let secret = descriptor.components.iter().any(|component| {
            selected.map_or(descriptor.components.len() == 1, |name| {
                component.name == name
            }) && component
                .provides
                .iter()
                .any(|interface| interface.split('@').next() == Some("cordis.secrets"))
        });
        if secret {
            let file = instance
                .get("config")
                .and_then(Value::as_table)
                .and_then(|config| config.get("file"))
                .and_then(Value::as_str)
                .ok_or("密钥实例需要 file 路径")?;
            let file = absolute(&plan.project, Path::new(file));
            if file.exists() {
                plan.secret_paths.insert(file.canonicalize()?);
            }
        }
    }
    Ok(())
}
