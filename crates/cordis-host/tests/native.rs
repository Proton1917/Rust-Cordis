//! DOC-15 M11、AU-9、AU-18：真实动态库、独立进程与旧编译器组成原生验收。

use std::{path::Path, process::Command};

#[test]
fn native_plugins_follow_core_lifecycle_and_process_isolation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = root.join("../../target/host-native");
    let status = Command::new(env!("CARGO"))
        .args(["run", "--manifest-path"])
        .arg(root.join("tests/driver/Cargo.toml"))
        .arg("--target-dir")
        .arg(target.join("driver"))
        .args(["--", "--artifacts-dir"])
        .arg(&target)
        .arg("--host")
        .arg(env!("CARGO_BIN_EXE_cordis-host"))
        .status()
        .expect("启动真实原生验收驱动");
    assert!(status.success(), "原生插件行为验收失败：{status}");
}
