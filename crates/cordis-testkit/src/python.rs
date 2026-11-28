//! DOC-15 M18/M20：把 Python 依赖安装到调用方拥有的测试目录。

use std::{env, fs, io, path::Path, process::Command};

/// 项目规则 9/10：统一解释器、pip 缓存与工作区临时目录，返回待执行的安装命令。
/// 优先使用 `CORDIS_PYTHON`，其次在 Conda 可用时走 `conda run -n base python`，
/// 无 Conda 的环境自动回退到当前 `python3`。
/// 调用方提供 requirements 与依赖目标，负责输出记录和现场清理；目录创建错误直接返回。
pub fn install_dependencies(
    workspace: &Path,
    requirements: &Path,
    destination: &Path,
) -> io::Result<Command> {
    let temporary = workspace.join("target/tmp");
    fs::create_dir_all(&temporary)?;
    let mut command = if let Some(python) = env::var_os("CORDIS_PYTHON").filter(|v| !v.is_empty()) {
        Command::new(python)
    } else if has_executable("conda") {
        let mut conda = Command::new("conda");
        conda.args(["run", "--no-capture-output", "-n", "base", "python"]);
        conda
    } else {
        Command::new("python3")
    };
    command
        .current_dir(workspace)
        .env("PIP_REQUIRE_VIRTUALENV", "false")
        .env("TMPDIR", temporary)
        .args([
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

fn has_executable(name: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&paths).any(|directory| {
        let candidate = directory.join(name);
        candidate.is_file()
    })
}
