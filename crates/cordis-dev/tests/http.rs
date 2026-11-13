//! ADR-0046 AU-19–AU-26：真实核心、插件与 TCP 客户端的外部接入验收。
#![cfg(target_os = "macos")]

use cordis_testkit::{
    build::{shared_build_lock, shared_target_dir, snapshot_artifacts},
    cleanup::{FixtureLease, SharedFixture, TestGuard},
    python::install_dependencies,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Tools {
    root: PathBuf,
    binaries: PathBuf,
    dependencies: PathBuf,
}

fn command(command: &mut Command, log: &Path) {
    if Path::new(command.get_program()).file_name() == Some(std::ffi::OsStr::new("cargo")) {
        let project = command
            .get_current_dir()
            .map(Path::to_owned)
            .unwrap_or_else(|| std::env::current_dir().unwrap());
        cordis_testkit::build::remap_paths(command, &project).unwrap();
    }
    let output = command.output().expect("启动 HTTP 验收命令");
    let mut bytes = output.stdout;
    bytes.extend(output.stderr);
    fs::write(log, &bytes).unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&bytes)
    );
}

/// ADR-0046：构建脚本 SDK 与真实提供方，安装及构建失败直接保留日志。
fn build_scripts(root: &Path, directory: &Path) {
    for package in [
        "sdks/typescript",
        "plugins/http-echo-ts",
        "plugins/kv-ts",
        "tests/http-sdk/process",
    ] {
        command(
            Command::new("npm")
                .current_dir(root.join(package))
                .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"])
                .env("npm_config_cache", root.join("target/npm-cache")),
            &directory.join(package.replace('/', "-") + "-install.log"),
        );
        command(
            Command::new("npm")
                .current_dir(root.join(package))
                .args(["run", "build"]),
            &directory.join(package.replace('/', "-") + "-build.log"),
        );
    }
}

/// 规则 11：全部 Cargo 调用使用共享目录，并由外层构建锁串行保护产物。
fn build_rust(root: &Path, directory: &Path) {
    let target = shared_target_dir(root);
    let mut programs = Command::new(env!("CARGO"));
    programs.current_dir(root).args(["build", "--release"]);
    for package in [
        "cordis-runtime",
        "cordis-host",
        "cordis-dev",
        "cordis-http",
        "http-echo",
        "cordis-secrets",
        "console-log",
        "kv-mem",
    ] {
        programs.args(["-p", package]);
    }
    command(
        programs.arg("--target-dir").arg(&target),
        &directory.join("programs-build.log"),
    );
    for (manifest, log) in [
        (
            "crates/cordis-runtime/fixtures/plugin/Cargo.toml",
            "management-build.log",
        ),
        (
            "tests/http-sdk/native/Cargo.toml",
            "native-fixtures-build.log",
        ),
        ("tests/http-sdk/versions/Cargo.toml", "versions-build.log"),
    ] {
        command(
            Command::new(env!("CARGO"))
                .current_dir(root)
                .args(["build", "--release", "--workspace", "--manifest-path"])
                .arg(root.join(manifest))
                .arg("--target-dir")
                .arg(&target),
            &directory.join(log),
        );
    }
}

fn tools() -> FixtureLease<Tools> {
    static TOOLS: SharedFixture<Tools> = SharedFixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    TOOLS.get_or_init(&root, "http-tools", |guard| {
        let directory = guard.directory("http-tools");
        fs::create_dir_all(&directory).unwrap();
        let _lock = shared_build_lock(&root).unwrap();
        build_scripts(&root, &directory);
        build_rust(&root, &directory);
        let binaries = snapshot_artifacts(
            guard,
            "http-tools",
            "release",
            &[
                "cordis",
                "cordis-host",
                "cordis-dev",
                "cordis-runtime-fixture",
                "libcordis_http.dylib",
                "libhttp_echo.dylib",
                "libcordis_secrets.dylib",
                "libconsole_log.dylib",
                "libhttp_test_native.dylib",
                "libkv_mem.dylib",
                "libhttp_echo_v1.dylib",
                "libhttp_echo_v2.dylib",
                "libhttp_echo_v3.dylib",
                "libhttp_echo_failed.dylib",
            ],
        )
        .unwrap();
        let dependencies = directory.join("dependencies");
        command(
            &mut install_dependencies(
                &root,
                &root.join("sdks/python/requirements.txt"),
                &dependencies,
            )
            .unwrap(),
            &directory.join("python-dependencies.log"),
        );
        Tools {
            root: root.clone(),
            binaries,
            dependencies,
        }
    })
}

fn scenario(name: &str) {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), name);
    let tools = tools().for_case(&case);
    let evidence = case.directory("http-sdk");
    fs::create_dir_all(&evidence).unwrap();
    let log = evidence.join("runner.log");
    command(
        Command::new("conda")
            .current_dir(&tools.root)
            .args(["run", "--no-capture-output", "-n", "base", "python"])
            .arg(tools.root.join("tests/http-sdk/run.py"))
            .arg("--workspace")
            .arg(&tools.root)
            .arg("--tools")
            .arg(&tools.binaries)
            .arg("--dependencies")
            .arg(&tools.dependencies)
            .arg("--evidence")
            .arg(&evidence)
            .arg("--scenario")
            .arg(name),
        &log,
    );
    eprintln!("HTTP 验收证据：{}", case.evidence_for(&evidence).display());
    case.success();
}

#[test]
fn http_routes_preserve_three_language_request_semantics() {
    scenario("routes");
}

#[test]
fn http_streams_transfer_large_files_with_bounded_memory() {
    scenario("files");
}

#[test]
fn aggregate_bytes_cross_all_languages_and_read_legacy_snapshots() {
    scenario("bytes");
}

#[test]
fn native_owned_threads_invoke_cancel_and_finish_host_operations() {
    scenario("threads");
}

#[test]
fn secret_grants_use_core_identity_and_service_environment_stays_public() {
    scenario("secrets");
}

#[test]
fn page_identity_proxy_and_application_shell_follow_real_connections() {
    scenario("web");
}

#[test]
fn http_scaffolds_build_and_serve_in_all_three_languages() {
    scenario("scaffold");
}

#[test]
fn byte_streams_cross_processes_and_close_cancelled_resources() {
    scenario("streams");
}

#[test]
fn http_replacement_rolls_back_under_traffic_and_faults_remain_local() {
    scenario("lifecycle");
}

#[test]
#[ignore = "RM-11 在独立测量期间固定运行一次"]
fn http_route_round_trip_latency() {
    scenario("latency");
}
