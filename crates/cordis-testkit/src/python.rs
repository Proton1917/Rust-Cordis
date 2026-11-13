//! DOC-15 M18/M20：通过 Conda base 把 Python 依赖安装到调用方拥有的测试目录。

use std::{fs, io, path::Path, process::Command};

/// 项目规则 9/10：统一解释器、pip 缓存与工作区临时目录，返回待执行的安装命令。
/// 调用方提供 requirements 与依赖目标，负责输出记录和现场清理；目录创建错误直接返回。
pub fn install_dependencies(
    workspace: &Path,
    requirements: &Path,
    destination: &Path,
) -> io::Result<Command> {
    let temporary = workspace.join("target/tmp");
    fs::create_dir_all(&temporary)?;
    let mut command = Command::new("conda");
    command
        .current_dir(workspace)
        .env("PIP_REQUIRE_VIRTUALENV", "false")
        .env("TMPDIR", temporary)
        .args([
            "run",
            "--no-capture-output",
            "-n",
            "base",
            "python",
            "-m",
            "pip",
            "install",
            "--disable-pip-version-check",
            "--no-deps",
            "--cache-dir",
        ])
        .arg(workspace.join("target/python-sdk-dependencies/pip-cache"))
        .arg("--target")
        .arg(destination)
        .arg("--requirement")
        .arg(requirements);
    Ok(command)
}
