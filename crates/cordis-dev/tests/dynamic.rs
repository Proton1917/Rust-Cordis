//! ADR-0047：四种真实提供方位置、资源引用与替换生命周期的动态调用验收。
#![cfg(target_os = "macos")]

#[path = "support/script_tools.rs"]
#[allow(dead_code)]
mod script_tools;

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
    base: FixtureLease<script_tools::Tools>,
    extra: PathBuf,
    dependencies: PathBuf,
}

fn checked(command: &mut Command, log: &Path) {
    let output = command.output().expect("执行动态调用验收命令");
    let mut bytes = output.stdout;
    bytes.extend(output.stderr);
    fs::write(log, &bytes).unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&bytes)
    );
}

/// AU-27–AU-30：当前提供方和失败候选沿同一个共享编译目录构建。
fn build_native(root: &Path, directory: &Path) {
    let target = shared_target_dir(root);
    checked(
        Command::new(env!("CARGO"))
            .current_dir(root)
            .args([
                "build",
                "-p",
                "kv-file",
                "-p",
                "cordis-secrets",
                "--target-dir",
            ])
            .arg(&target),
        &directory.join("file-provider.log"),
    );
    for (manifest, features, log) in [
        ("tests/dynamic-sdk/native/Cargo.toml", "", "probe-build.log"),
        (
            "crates/cordis-runtime/fixtures/kv-evolution/Cargo.toml",
            "fails",
            "failed-provider.log",
        ),
    ] {
        let mut command = Command::new(env!("CARGO"));
        command
            .current_dir(root)
            .args(["build", "--manifest-path"])
            .arg(root.join(manifest))
            .arg("--target-dir")
            .arg(&target);
        if !features.is_empty() {
            command.args(["--features", features]);
        }
        checked(&mut command, &directory.join(log));
    }
}

fn build_scripts(root: &Path, directory: &Path) {
    for package in [
        "sdks/typescript",
        "plugins/kv-ts",
        "tests/dynamic-sdk/script",
    ] {
        checked(
            Command::new("npm")
                .current_dir(root.join(package))
                .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"])
                .env("npm_config_cache", root.join("target/npm-cache")),
            &directory.join(package.replace('/', "-") + "-install.log"),
        );
        checked(
            Command::new("npm")
                .current_dir(root.join(package))
                .args(["run", "build"]),
            &directory.join(package.replace('/', "-") + "-build.log"),
        );
    }
}

fn tools() -> FixtureLease<Tools> {
    static TOOLS: SharedFixture<Tools> = SharedFixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    TOOLS.get_or_init(&root, "dynamic-tools", |guard| {
        let base = script_tools::tools().for_case(guard);
        let directory = guard.directory("dynamic-tools");
        fs::create_dir_all(&directory).unwrap();
        let _lock = shared_build_lock(&root).unwrap();
        build_native(&root, &directory);
        build_scripts(&root, &directory);
        let dependencies = directory.join("dependencies");
        checked(
            &mut install_dependencies(
                &root,
                &root.join("sdks/python/requirements.txt"),
                &dependencies,
            )
            .unwrap(),
            &directory.join("python-dependencies.log"),
        );
        let extra = snapshot_artifacts(
            guard,
            "dynamic-tools",
            "debug",
            &[
                "libdynamic_test_native.dylib",
                "libkv_file.dylib",
                "libkv_evolution.dylib",
                "libcordis_secrets.dylib",
            ],
        )
        .unwrap();
        Tools {
            base,
            extra,
            dependencies,
        }
    })
}

fn scenario(name: &str) {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), name);
    let tools = tools().for_case(&case);
    let evidence = case.directory("dynamic-sdk");
    fs::create_dir_all(&evidence).unwrap();
    checked(
        Command::new("conda")
            .current_dir(&tools.base.root)
            .args(["run", "--no-capture-output", "-n", "base", "python"])
            .arg(tools.base.root.join("tests/dynamic-sdk/run.py"))
            .arg("--workspace")
            .arg(&tools.base.root)
            .arg("--tools")
            .arg(&tools.base.binaries)
            .arg("--extra")
            .arg(&tools.extra)
            .arg("--dependencies")
            .arg(&tools.dependencies)
            .arg("--evidence")
            .arg(&evidence)
            .arg("--scenario")
            .arg(name),
        &evidence.join("runner.log"),
    );
    eprintln!("动态调用证据：{}", case.evidence_for(&evidence).display());
    case.success();
}

#[test]
fn dynamic_calls_match_typed_calls_and_own_stream_resources() {
    scenario("matrix");
}

#[test]
fn dynamic_dependencies_rebuild_restore_and_become_pending() {
    scenario("lifecycle");
}

#[test]
fn script_dynamic_calls_reject_unowned_resource_tags() {
    scenario("scripts");
}
