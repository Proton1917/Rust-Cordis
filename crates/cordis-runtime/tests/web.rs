//! DOC-15 M20、AU-7/14/15：当前源码构建网页宿主，真实浏览器场景由 TestGuard 归档。
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
    examples: PathBuf,
    dependencies: PathBuf,
}

/// Y-3：每个命令保留实际输出，失败由对应场景的守卫保存现场。
fn checked(command: &mut Command, log: &Path) {
    let result = command.output().expect("执行网页验收命令");
    let mut output = result.stdout;
    output.extend_from_slice(&result.stderr);
    fs::write(log, &output).unwrap();
    assert!(
        result.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output)
    );
}

/// 项目规则 11：外部快照沿显式环境读取，常规运行自建并持锁保存当前产物。
fn tools() -> FixtureLease<Tools> {
    static TOOLS: SharedFixture<Tools> = SharedFixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    TOOLS.get_or_init(&root, "web-sdk-tools", |guard| {
        let directory = guard.directory("web-sdk-tools");
        fs::create_dir_all(&directory).unwrap();
        let binaries = match env::var_os("CORDIS_WEB_TOOLS") {
            Some(directory) => PathBuf::from(directory),
            None => build(&root, guard, &directory),
        };
        let examples = match env::var_os("CORDIS_WEB_EXAMPLES") {
            Some(directory) => PathBuf::from(directory),
            None => examples(&root, &binaries, &directory),
        };
        let dependencies = match env::var_os("CORDIS_TEST_DEPENDENCIES") {
            Some(directory) => PathBuf::from(directory),
            None => dependencies(&root, &directory),
        };
        Tools {
            root: root.clone(),
            binaries,
            examples,
            dependencies,
        }
    })
}

/// M20：网页 SDK 构建先于 HTTP 编译，Cargo 与夹具使用同一共享缓存。
fn build(root: &Path, guard: &TestGuard, directory: &Path) -> PathBuf {
    let _lock = shared_build_lock(root).unwrap();
    let target = shared_target_dir(root);
    build_sdk(root, directory);
    checked(
        Command::new(env!("CARGO"))
            .current_dir(root)
            .args([
                "build",
                "-p",
                "cordis-runtime",
                "-p",
                "cordis-host",
                "-p",
                "cordis-dev",
                "-p",
                "cordis-http",
                "-p",
                "kv-mem",
                "-p",
                "cordis-watch",
                "--target-dir",
            ])
            .arg(&target),
        &directory.join("programs-build.log"),
    );
    checked(
        Command::new(env!("CARGO"))
            .current_dir(root)
            .args(["build", "--manifest-path"])
            .arg(root.join("crates/cordis-runtime/fixtures/plugin/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target),
        &directory.join("management-build.log"),
    );
    snapshot_artifacts(
        guard,
        "web-sdk-tools",
        "debug",
        &[
            "cordis",
            "cordis-host",
            "cordis-dev",
            "cordis-runtime-fixture",
            "libcordis_http.dylib",
            "libcordis_watch.dylib",
            "libkv_mem.dylib",
        ],
    )
    .unwrap()
}

/// M20：统一 Node 构建入口同时生成浏览器 SDK 和 HTTP 内置资源。
fn build_sdk(root: &Path, directory: &Path) {
    let sdk = root.join("sdks/typescript");
    checked(
        Command::new("npm")
            .current_dir(&sdk)
            .env("npm_config_cache", root.join("target/npm-cache"))
            .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"]),
        &directory.join("sdk-dependencies.log"),
    );
    checked(
        Command::new("npm")
            .current_dir(&sdk)
            .env("npm_config_cache", root.join("target/npm-cache"))
            .args(["run", "build"]),
        &directory.join("sdk-build.log"),
    );
}

/// M20：示例源码在守卫目录内构建，动态库和 Python 模块来自同次正式源码。
fn examples(root: &Path, binaries: &Path, directory: &Path) -> PathBuf {
    let destination = directory.join("examples");
    fs::create_dir(&destination).unwrap();
    let source = root.join("examples/web");
    for file in [
        "package.json",
        "package-lock.json",
        "build.mjs",
        "prepare-backends.mjs",
    ] {
        fs::copy(source.join(file), destination.join(file)).unwrap();
    }
    checked(
        Command::new("cp")
            .arg("-R")
            .arg(source.join("source"))
            .arg(destination.join("source")),
        &directory.join("example-source.log"),
    );
    checked(
        Command::new("npm")
            .current_dir(&destination)
            .env("npm_config_cache", root.join("target/npm-cache"))
            .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"]),
        &directory.join("example-dependencies.log"),
    );
    checked(
        Command::new("npm")
            .current_dir(&destination)
            .env("npm_config_cache", root.join("target/npm-cache"))
            .args(["run", "build"]),
        &directory.join("example-build.log"),
    );
    checked(
        Command::new("node")
            .current_dir(&destination)
            .arg("prepare-backends.mjs")
            .arg("--workspace")
            .arg(root)
            .arg("--native")
            .arg(binaries.join("libkv_mem.dylib"))
            .arg("--http")
            .arg(binaries.join("libcordis_http.dylib")),
        &directory.join("example-backends.log"),
    );
    destination.join("source")
}

/// M18/M20：CBOR 依赖安装到守卫拥有的目录，复用项目缓存和现行 Python 环境。
fn dependencies(root: &Path, directory: &Path) -> PathBuf {
    let destination = directory.join("dependencies");
    checked(
        &mut install_dependencies(
            root,
            &root.join("sdks/python/requirements.txt"),
            &destination,
        )
        .unwrap(),
        &directory.join("python-dependencies.log"),
    );
    destination
}

/// AU-7/14/15：Python 持有并关闭真实进程与页面，成功后由 Rust 守卫归档证据。
fn scenario(name: &str) {
    let case = TestGuard::for_package(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &format!("web-{name}"),
    );
    let tools = tools().for_case(&case);
    let directory = case.directory("web-sdk");
    fs::create_dir_all(&directory).unwrap();
    let evidence = directory.join("case");
    let log = directory.join("run.log");
    checked(
        Command::new("conda")
            .current_dir(&tools.root)
            .args(["run", "--no-capture-output", "-n", "base", "python"])
            .arg(tools.root.join("tests/web-sdk/run.py"))
            .arg("--tools")
            .arg(&tools.binaries)
            .arg("--examples")
            .arg(&tools.examples)
            .arg("--dependencies")
            .arg(&tools.dependencies)
            .arg("--evidence")
            .arg(&evidence)
            .arg("--scenario")
            .arg(name),
        &log,
    );
    eprintln!("网页验收证据：{}", case.evidence_for(&evidence).display());
    case.success();
}

#[test]
fn pure_web_replacement_preserves_each_page_state_and_rolls_back() {
    scenario("pure-replace");
}
#[test]
fn fullstack_replacement_commits_or_restores_backend_and_pages() {
    scenario("fullstack");
}
#[test]
fn page_realms_rebuild_dependencies_and_close_independently() {
    scenario("realm-close");
}
#[test]
fn web_pages_recover_after_each_language_provider_returns() {
    scenario("dependencies");
}
#[test]
fn http_restart_reconnects_existing_pages_at_same_origin() {
    scenario("http-restart");
}
#[test]
fn invalid_channel_frames_and_flood_preserve_other_pages() {
    scenario("channel-faults");
}
#[test]
fn generated_web_plugin_runs_in_the_browser() {
    scenario("scaffold");
}
#[test]
fn web_batches_restore_or_publish_backends_and_pages() {
    scenario("batch");
}

#[test]
fn watch_replaces_deployment_and_preserves_two_page_projections() {
    scenario("watch-replace");
}
