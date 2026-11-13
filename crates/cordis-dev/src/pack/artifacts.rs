//! DOC-15 §12.2：Cargo 的结构化构建消息提供发布目标，目标机器类型另由产物检查。

use crate::{Result, build_paths, tooling};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Artifacts {
    pub libraries: BTreeMap<String, PathBuf>,
    pub binaries: BTreeMap<String, PathBuf>,
}

/// AU-12：Cargo 错误原样返回；已有清单以实际 release 产物作为复制来源。
pub async fn build(project: &Path, target: &str, target_dir: &Path) -> Result<Artifacts> {
    let manifest = project.join("Cargo.toml");
    let output = match std::fs::metadata(manifest) {
        Ok(_) => {
            tooling::command(
                vec![
                    "env".into(),
                    format!(
                        "CARGO_ENCODED_RUSTFLAGS={}",
                        build_paths::encoded_flags(project)?
                    ),
                    tooling::executable("cargo", None)?
                        .to_string_lossy()
                        .into_owned(),
                    "build".into(),
                    "--release".into(),
                    "--workspace".into(),
                    "--target".into(),
                    target.into(),
                    "--target-dir".into(),
                    target_dir.to_string_lossy().into_owned(),
                    "--message-format=json-render-diagnostics".into(),
                ],
                project,
                Duration::from_secs(600),
            )
            .await?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let mut artifacts = Artifacts {
        libraries: BTreeMap::new(),
        binaries: BTreeMap::new(),
    };
    for message in serde_json::Deserializer::from_slice(&output).into_iter::<Value>() {
        let message = message?;
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        record(&mut artifacts, &message)?;
    }
    Ok(artifacts)
}

/// DOC-15 §12.2：可执行目标与 cdylib 分别服务进程和原生插口。
fn record(artifacts: &mut Artifacts, message: &Value) -> Result<()> {
    if message["target"]["kind"]
        .as_array()
        .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        && let Some(executable) = message["executable"].as_str()
    {
        let path = PathBuf::from(executable);
        artifacts.binaries.insert(filename(&path)?, path);
    }
    if message["target"]["crate_types"]
        .as_array()
        .is_some_and(|types| types.iter().any(|kind| kind == "cdylib"))
        && let Some(filenames) = message["filenames"].as_array()
    {
        for filename in filenames.iter().filter_map(Value::as_str) {
            let path = PathBuf::from(filename);
            if matches!(
                path.extension().and_then(|part| part.to_str()),
                Some("dylib" | "so")
            ) {
                artifacts.libraries.insert(self::filename(&path)?, path);
            }
        }
    }
    Ok(())
}

pub fn filename(path: &Path) -> Result<String> {
    Ok(path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("产物文件名需要 UTF-8")?
        .to_owned())
}
