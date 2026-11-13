//! DOC-15 M11、AU-9、AU-18：真实动态库、独立进程与旧编译器组成原生验收。

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

struct Driver {
    target: PathBuf,
    executable: PathBuf,
}

static DRIVER: OnceLock<Result<Driver, String>> = OnceLock::new();

fn build_driver() -> Result<Driver, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = root.parent().and_then(Path::parent).unwrap();
    let target = workspace
        .join("target/host-native")
        .join(std::process::id().to_string());
    let shared = cordis_testkit::build::shared_target_dir(workspace);
    let status = Command::new(env!("CARGO"))
        .args(["run", "--manifest-path"])
        .arg(root.join("tests/driver/Cargo.toml"))
        .arg("--target-dir")
        .arg(&shared)
        .args(["--", "--build-only", "--artifacts-dir"])
        .arg(&target)
        .arg("--host")
        .arg(env!("CARGO_BIN_EXE_cordis-host"))
        .status()
        .map_err(|error| format!("启动原生样例构建失败：{error}"))?;
    if !status.success() {
        return Err(format!("原生样例构建失败：{status}"));
    }
    Ok(Driver {
        executable: shared.join("debug/cordis-host-driver"),
        target,
    })
}

fn scenario(name: &str) {
    let driver = DRIVER
        .get_or_init(build_driver)
        .as_ref()
        .unwrap_or_else(|error| panic!("原生场景 {name}：{error}"));
    let status = Command::new(&driver.executable)
        .args(["--reuse-artifacts", "--artifacts-dir"])
        .arg(&driver.target)
        .arg("--host")
        .arg(env!("CARGO_BIN_EXE_cordis-host"))
        .arg(name)
        .status()
        .unwrap_or_else(|error| panic!("启动原生场景 {name} 失败：{error}"));
    assert!(status.success(), "原生场景 {name} 失败：{status}");
}

macro_rules! native_scenarios {
    ($($test:ident => $case:literal),+ $(,)?) => {
        $(#[test] fn $test() { scenario($case); })+
    };
}

native_scenarios! {
    same_group_typed_calls => "basic-same",
    cross_group_generic_calls => "basic-cross",
    same_group_composition => "scoped-same",
    cross_group_composition => "scoped-cross",
    native_library_swap_and_rollback => "swap",
    retired_image_threshold_restarts_group => "threshold",
    shared_directory_allocates_monotonic_copies => "allocation",
    service_panic_preserves_host => "panic",
    apply_panic_reports_failure => "apply-panic",
    abort_isolates_group => "abort",
    segmentation_fault_isolates_group => "segfault",
    stack_overflow_isolates_group => "stack",
    process_exit_isolates_group => "exit",
    endless_computation_isolates_group => "spin",
}
