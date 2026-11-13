//! ADR-0046 §3.10：按插件实际服务声明识别密钥路径，复制阶段跳过这些文件。

use super::{Artifacts, Plan, Result, absolute, artifacts, plugins, string, tooling};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use toml::{Table, Value};

/// 发布前读取原生描述符，装配共用本轮结果；文件内容始终由密钥提供方读取。
pub(super) async fn sources(plan: &mut Plan, artifacts: &Artifacts) -> Result<()> {
    let directory = plan
        .configuration
        .get("runtime")
        .and_then(Value::as_table)
        .map(|runtime| string(runtime, "plugin_dir"))
        .transpose()?
        .flatten()
        .unwrap_or("plugins");
    let directory = absolute(&plan.project, Path::new(directory));
    for (reference, instances) in plugins::requested(&plan.configuration)? {
        let original = absolute(&directory, Path::new(&reference));
        let Some(source) = native_source(plan, artifacts, &original)? else {
            continue;
        };
        let source = source.canonicalize()?;
        let descriptor = if let Some(descriptor) = plan.descriptions.get(&source) {
            descriptor.clone()
        } else {
            plan.platform.binary(&source)?;
            let descriptor =
                tooling::describe(&plan.host, &source, &plan.project, Duration::from_secs(30))
                    .await?;
            plan.descriptions.insert(source, descriptor.clone());
            descriptor
        };
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
    }
    Ok(())
}

fn native_source(plan: &Plan, artifacts: &Artifacts, original: &Path) -> Result<Option<PathBuf>> {
    let original = if original.is_dir() {
        let manifest: Table =
            toml::from_str(&fs::read_to_string(original.join("cordis-plugin.toml"))?)?;
        if string(&manifest, "socket")? != Some("native") {
            return Ok(None);
        }
        absolute(
            original,
            Path::new(string(&manifest, "library")?.ok_or("原生清单需要 library")?),
        )
    } else {
        original.to_owned()
    };
    let name = artifacts::filename(&original)?;
    let filename = if original.extension().is_some() {
        name
    } else {
        plan.platform.library(&name)
    };
    Ok(Some(
        artifacts
            .libraries
            .get(&filename)
            .cloned()
            .unwrap_or(original),
    ))
}
