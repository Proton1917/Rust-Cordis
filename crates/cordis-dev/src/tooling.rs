//! Q0、CP-4/CP-7：开发工具通过受控子进程使用编译器、核心与宿主。

use crate::Result;
use cordis_up::{Limits, payload};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub fn executable(name: &str, directory: Option<&Path>) -> Result<PathBuf> {
    if let Some(directory) = directory {
        return Ok(directory.join(name).canonicalize()?);
    }
    let current = std::env::current_exe()?;
    let sibling = current.parent().expect("可执行文件父目录").join(name);
    if sibling.is_file() {
        return Ok(sibling);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(format!("程序 {name} 安装路径缺席").into())
}

/// CP-4/CP-7：截止时间和输出上限交给既有进程组拥有者。
pub async fn command(command: Vec<String>, project: &Path, timeout: Duration) -> Result<Vec<u8>> {
    Ok(cordis_process::run_capture(&command, project, timeout, Limits::DEFAULT.max_frame).await?)
}

/// Q0：宿主打开库并返回经过 ABI 握手的描述符。
pub async fn describe(
    host: &Path,
    library: &Path,
    project: &Path,
    timeout: Duration,
) -> Result<payload::Plugin> {
    let output = command(
        vec![
            host.to_string_lossy().into_owned(),
            "--describe".into(),
            library.to_string_lossy().into_owned(),
            "--cache-dir".into(),
            project
                .join(".cordis/lib/discovery")
                .to_string_lossy()
                .into_owned(),
        ],
        project,
        timeout,
    )
    .await?;
    Ok(serde_json::from_slice(&output)?)
}
