//! 项目规则 11：测试构建共用缓存，并在复制候选产物期间持有跨进程锁。

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// 发布验证的编译命令采用与 pack 相同的路径映射，产物目录继续由调用方选择。
pub fn remap_paths(command: &mut std::process::Command, project: &Path) -> io::Result<()> {
    command.env(
        "CARGO_ENCODED_RUSTFLAGS",
        super::build_paths::encoded_flags(project)?,
    );
    Ok(())
}

/// 项目规则 11：解析当前会话编译根；相对环境路径以工作区为基准。
pub fn target_root(workspace: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(directory) => workspace.join(directory),
        None => workspace.join("target"),
    }
}

/// 项目规则 11：全部测试夹具和测量共用当前编译根的 shared 目录。
pub fn shared_target_dir(workspace: &Path) -> PathBuf {
    target_root(workspace).join("shared")
}

/// 项目规则 11：构建与产物复制在同一锁作用域内执行，File 释放时解除锁。
/// 调用方在启动 Cargo 前获取，构建或复制失败由调用方报告。
pub fn shared_build_lock(workspace: &Path) -> io::Result<File> {
    let directory = shared_target_dir(workspace);
    fs::create_dir_all(&directory)?;
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".cordis-build.lock"))?;
    lock.lock()?;
    Ok(lock)
}

/// 项目规则 11：持锁复制本次实际运行的产物到证据目录，保留不同特性及编译器结果。
pub fn snapshot_artifacts(
    guard: &crate::cleanup::TestGuard,
    label: &str,
    profile: &str,
    names: &[&str],
) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let workspace = guard.workspace();
    let destination = guard.track(workspace.join("target/test-artifacts").join(format!(
        "{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir_all(&destination)?;
    let source = shared_target_dir(workspace).join(profile);
    for name in names {
        fs::copy(source.join(name), destination.join(name))?;
    }
    Ok(destination)
}
