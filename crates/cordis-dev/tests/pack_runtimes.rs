//! DOC-15 AU-12、M21：完整发行运行时与真实脚本插件在移动后的发布包运行。
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use cordis_testkit::{
    build::{shared_build_lock, shared_target_dir, snapshot_artifacts},
    cleanup::TestGuard,
    python::install_dependencies,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

#[test]
fn complete_python_and_node_packages_run_native_extensions_and_real_services() {
    let root = env::var_os("CORDIS_TEST_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap()
        });
    let case = TestGuard::new(&root, "m21-pack-runtimes");
    let container = case.directory("pack-runtimes");
    fs::create_dir_all(container.join("tmp")).unwrap();
    let tools = env::var_os("CORDIS_PACK_TOOLS")
        .map(PathBuf::from)
        .unwrap_or_else(|| tools(&root, &case, &container));
    let dependencies = env::var_os("CORDIS_TEST_DEPENDENCIES")
        .map(PathBuf::from)
        .unwrap_or_else(|| python_dependencies(&root, &container));
    run_case(&root, &container, &tools, &dependencies);
    eprintln!(
        "独立运行时证据：{}",
        case.evidence_for(&container).display()
    );
    case.success();
}

/// AU-12：默认输入由运行入口准备，显式环境路径沿用调用方指定的运行时快照。
fn run_case(root: &Path, container: &Path, tools: &Path, dependencies: &Path) {
    let mut command = Command::new("conda");
    command
        .current_dir(root)
        .env("TMPDIR", container.join("tmp"))
        .args(["run", "--no-capture-output", "-n", "base", "python"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/pack-runtimes/run.py"))
        .arg("--workspace")
        .arg(root)
        .arg("--evidence")
        .arg(container.join("case"))
        .arg("--tools")
        .arg(tools)
        .arg("--dev")
        .arg(env!("CARGO_BIN_EXE_cordis-dev"))
        .arg("--python-dependencies")
        .arg(dependencies);
    if let Some(runtimes) = env::var_os("CORDIS_PACK_RUNTIMES") {
        command.arg("--runtimes").arg(runtimes);
    }
    checked(&mut command, &container.join("run.log"));
}

/// M18/M21：CPython 3.12的扩展安装到守卫目录，安装失败保留日志与现场。
fn python_dependencies(root: &Path, directory: &Path) -> PathBuf {
    let destination = directory.join("python-dependencies");
    let mut command = install_dependencies(
        root,
        &root.join("sdks/python/requirements.txt"),
        &destination,
    )
    .unwrap();
    command.args(["--python-version", "3.12", "--only-binary=:all:"]);
    checked(&mut command, &directory.join("python-dependencies.log"));
    destination
}

/// Y-3：每条准备命令保留实际输出，失败由测试守卫保留对应现场。
fn checked(command: &mut Command, path: &Path) {
    let output = command.output().unwrap();
    let mut log = output.stdout;
    log.extend(output.stderr);
    fs::write(path, &log).unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&log));
}

/// 项目规则11、AU-12：常规测试自建工具，实际验收可复用显式指定的有效快照。
fn tools(root: &Path, case: &TestGuard, directory: &Path) -> PathBuf {
    let _lock = shared_build_lock(root).unwrap();
    let target = shared_target_dir(root);
    let mut commands = [Command::new(env!("CARGO")), Command::new(env!("CARGO"))];
    commands[0].args([
        "build",
        "--release",
        "-p",
        "cordis-runtime",
        "-p",
        "cordis-host",
    ]);
    commands[1]
        .args(["build", "--release", "--manifest-path"])
        .arg(root.join("crates/cordis-runtime/fixtures/plugin/Cargo.toml"));
    for (index, command) in commands.iter_mut().enumerate() {
        command.current_dir(root).arg("--target-dir").arg(&target);
        cordis_testkit::build::remap_paths(command, root).unwrap();
        checked(command, &directory.join(format!("tools-{index}.log")));
    }
    snapshot_artifacts(
        case,
        "pack-runtime-tools",
        "release",
        &["cordis", "cordis-host", "cordis-runtime-fixture"],
    )
    .unwrap()
}
