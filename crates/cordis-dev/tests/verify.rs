//! DOC-11 §8、M13：开发命令通过真实核心与动态库执行契约及准入验证。

use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

struct Artifacts {
    root: PathBuf,
    tools: PathBuf,
    bad: PathBuf,
    aborted: PathBuf,
}

fn checked(command: &mut Command) -> Output {
    let output = command.output().expect("运行验收命令");
    assert!(
        output.status.success(),
        "命令失败 {command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Y-3：真实程序、被测库和套件由测试自身构建，两个负向库分别使用对应构建配置。
fn artifacts() -> &'static Artifacts {
    static ARTIFACTS: OnceLock<Artifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let directory = root.join("target/verification-tests");
        let tools = directory.join("tools");
        checked(
            Command::new("cargo")
                .current_dir(&root)
                .args([
                    "build",
                    "-p",
                    "cordis-runtime",
                    "-p",
                    "cordis-host",
                    "-p",
                    "kv-mem",
                    "-p",
                    "kv-file",
                    "-p",
                    "kv-contract",
                    "--target-dir",
                ])
                .arg(&tools),
        );
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kv-broken/Cargo.toml");
        let bad = directory.join("broken");
        checked(
            Command::new("cargo")
                .args(["build", "--manifest-path"])
                .arg(&fixture)
                .arg("--target-dir")
                .arg(&bad),
        );
        let aborted = directory.join("aborted");
        checked(
            Command::new("cargo")
                .args(["build", "--manifest-path"])
                .arg(&fixture)
                .args(["--features", "abort", "--target-dir"])
                .arg(&aborted),
        );
        Artifacts {
            root,
            tools: tools.join("debug"),
            bad: library(&bad.join("debug"), "kv_broken"),
            aborted: library(&aborted.join("debug"), "kv_broken"),
        }
    })
}

fn library(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!(
        "{}{}{}",
        env::consts::DLL_PREFIX,
        name,
        env::consts::DLL_SUFFIX
    ))
}

fn project(artifacts: &Artifacts) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = artifacts
        .root
        .join("target/verification-tests")
        .join(format!(
            "project-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir(&directory).unwrap();
    directory
}

fn verify(artifacts: &Artifacts, project: &Path, plugin: &Path, config: Option<&Path>) -> Output {
    let mut paths = vec![artifacts.tools.clone()];
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let mut command = Command::new(env!("CARGO_BIN_EXE_cordis-dev"));
    command
        .env("PATH", env::join_paths(paths).unwrap())
        .arg("verify")
        .arg("--project")
        .arg(project)
        .arg("--plugin")
        .arg(plugin)
        .arg("--suite")
        .arg(library(&artifacts.tools, "kv_contract"));
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    command.output().unwrap()
}

fn receipt(plugin: &Path) -> PathBuf {
    let mut path = plugin.as_os_str().to_owned();
    path.push(".verified.json");
    path.into()
}

#[test]
fn memory_and_file_plugins_pass_real_contract_and_receive_gate_evidence() {
    let artifacts = artifacts();
    let project = project(artifacts);
    let config = project.join("file.toml");
    fs::write(&config, "path = 'store.cbor'\n").unwrap();
    for (name, config) in [("kv_mem", None), ("kv_file", Some(config.as_path()))] {
        let plugin = library(&project, name);
        fs::copy(library(&artifacts.tools, name), &plugin).unwrap();
        let output = verify(artifacts, &project, &plugin, config);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .filter(|line| line.starts_with("PASS "))
                .count(),
            8
        );
        let stamp: Value = serde_json::from_slice(&fs::read(receipt(&plugin)).unwrap()).unwrap();
        assert_eq!(stamp["suites"][0]["assertions"], 8);
        assert_eq!(
            fs::read(&plugin).unwrap(),
            fs::read(stamp["verified_copy"].as_str().unwrap()).unwrap()
        );
        checked(
            Command::new(artifacts.tools.join("cordis-host"))
                .arg("--describe")
                .arg(&plugin)
                .arg("--cache-dir")
                .arg(project.join("gate-cache"))
                .arg("--verified-dir")
                .arg(project.join(".cordis/verified"))
                .arg("--require-verified"),
        );
    }
}

#[test]
fn contract_violation_and_provider_exit_produce_failure_without_receipt() {
    let artifacts = artifacts();
    let project = project(artifacts);
    let bad = library(&project, "kv_broken");
    let aborted = library(&project, "kv_aborted");
    fs::copy(&artifacts.bad, &bad).unwrap();
    fs::copy(&artifacts.aborted, &aborted).unwrap();
    let failed = verify(artifacts, &project, &bad, None);
    assert_eq!(failed.status.code(), Some(1));
    assert!(
        String::from_utf8(failed.stdout)
            .unwrap()
            .contains("FAIL suite.put.bytes")
    );
    assert!(!receipt(&bad).exists());
    let exited = verify(artifacts, &project, &aborted, None);
    assert_eq!(exited.status.code(), Some(1));
    assert!(
        String::from_utf8(exited.stderr)
            .unwrap()
            .contains("验证核心")
    );
    assert!(!receipt(&aborted).exists());
}

#[test]
fn verification_sequences_continue_after_existing_high_entries() {
    let artifacts = artifacts();
    let project = project(artifacts);
    let sessions = project.join("target/verify");
    let copies = project.join(".cordis/verified");
    fs::create_dir_all(sessions.join("7")).unwrap();
    fs::create_dir_all(&copies).unwrap();
    let plugin = library(&project, "kv_mem");
    fs::copy(library(&artifacts.tools, "kv_mem"), &plugin).unwrap();
    fs::copy(
        &plugin,
        copies.join(format!("kv-7{}", env::consts::DLL_SUFFIX)),
    )
    .unwrap();
    let result = verify(artifacts, &project, &plugin, None);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(sessions.join("8/contract-report.json").is_file());
    let stamp: Value = serde_json::from_slice(&fs::read(receipt(&plugin)).unwrap()).unwrap();
    assert_eq!(
        Path::new(stamp["verified_copy"].as_str().unwrap())
            .file_name()
            .unwrap(),
        format!("kv-8{}", env::consts::DLL_SUFFIX).as_str()
    );
}
