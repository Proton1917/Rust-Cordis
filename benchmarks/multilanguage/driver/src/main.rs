//! DOC-15 RM-7/8/9：当前源码构建、固定测量与现场释放由同一守卫拥有。

use cordis_testkit::{
    build::{shared_build_lock, shared_target_dir, snapshot_artifacts},
    cleanup::TestGuard,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn checked(command: &mut Command, log: &Path, temporary: &Path) {
    let output = command
        .env("TMPDIR", temporary)
        .output()
        .expect("执行测量命令");
    let mut bytes = output.stdout;
    bytes.extend(output.stderr);
    fs::write(log, &bytes).unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&bytes)
    );
}

fn library(name: &str) -> String {
    format!(
        "{}{name}{}",
        env::consts::DLL_PREFIX,
        env::consts::DLL_SUFFIX
    )
}

/// 项目规则11：构建及运行副本复制共同持有共享构建锁。
fn programs(root: &Path, guard: &TestGuard, work: &Path, mode: &str) -> PathBuf {
    let _lock = shared_build_lock(root).unwrap();
    let target = shared_target_dir(root);
    let temporary = work.join("tmp");
    let packages: &[&str] = match mode {
        "rpc" => &["cordis-runtime", "cordis-host", "kv-mem"],
        "replace" => &["cordis-runtime", "cordis-host", "kv-mem", "kv-file"],
        "direct" => &["cordis-host"],
        "prepare" => &["cordis-runtime", "cordis-host", "kv-mem", "kv-file"],
        _ => unreachable!(),
    };
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .args(["build", "--release", "--target-dir"])
        .arg(&target);
    for package in packages {
        command.args(["-p", package]);
    }
    checked(&mut command, &work.join("programs-build.log"), &temporary);
    let mut names = vec!["cordis-host".to_owned()];
    if mode != "direct" {
        names.extend(["cordis".into(), library("kv_mem")]);
    }
    if matches!(mode, "rpc" | "prepare") {
        build_manifest(
            root,
            &root.join("benchmarks/multilanguage/probe/Cargo.toml"),
            &target,
            None,
            &work.join("probe-build.log"),
            &temporary,
        );
        names.push(library("cordis_latency_probe"));
    }
    if matches!(mode, "replace" | "prepare") {
        build_manifest(
            root,
            &root.join("crates/cordis-runtime/fixtures/plugin/Cargo.toml"),
            &target,
            None,
            &work.join("management-build.log"),
            &temporary,
        );
        names.extend(["cordis-runtime-fixture".into(), library("kv_file")]);
    }
    snapshot_artifacts(
        guard,
        "phase3-programs",
        "release",
        &names.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .unwrap()
}

fn build_manifest(
    root: &Path,
    manifest: &Path,
    target: &Path,
    compiler: Option<&str>,
    log: &Path,
    temporary: &Path,
) {
    let mut command = Command::new("cargo");
    if let Some(compiler) = compiler {
        command.arg(compiler);
    }
    checked(
        command
            .current_dir(root)
            .args(["build", "--release", "--manifest-path"])
            .arg(manifest)
            .arg("--target-dir")
            .arg(target),
        log,
        temporary,
    );
}

/// RM-9：M9历史源码来自Git归档，归档流直接交给工作区内的解包目录。
fn baseline_source(root: &Path, guard: &TestGuard, work: &Path) {
    let directory = root.join("target/direct-call-baseline");
    if directory.exists() {
        return;
    }
    guard.track(directory.clone());
    fs::create_dir_all(&directory).unwrap();
    let mut archive = Command::new("git")
        .current_dir(root)
        .args(["archive", "dc93da1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let output = Command::new("tar")
        .args(["-x", "-C"])
        .arg(directory)
        .stdin(Stdio::from(archive.stdout.take().unwrap()))
        .output()
        .unwrap();
    fs::write(work.join("baseline-source.log"), &output.stderr).unwrap();
    assert!(output.status.success());
    assert!(archive.wait().unwrap().success());
}

/// RM-9：consumer与同一provider使用1.85，驱动保留项目固定编译器。
fn direct_tools(root: &Path, guard: &TestGuard, work: &Path) -> PathBuf {
    baseline_source(root, guard, work);
    let _lock = shared_build_lock(root).unwrap();
    let target = shared_target_dir(root);
    let temporary = work.join("tmp");
    for name in ["baseline", "current"] {
        build_manifest(
            root,
            &root.join(format!("benchmarks/direct-call/{name}/Cargo.toml")),
            &target,
            Some("+1.85.0"),
            &work.join(format!("{name}-build.log")),
            &temporary,
        );
    }
    checked(
        Command::new("cargo")
            .current_dir(root)
            .args(["+1.85.0", "build", "--release", "--manifest-path"])
            .arg(root.join("crates/cordis-host/tests/fixtures/Cargo.toml"))
            .args(["-p", "loader-fixture", "--target-dir"])
            .arg(&target),
        &work.join("provider-build.log"),
        &temporary,
    );
    build_manifest(
        root,
        &root.join("benchmarks/direct-call/driver/Cargo.toml"),
        &target,
        None,
        &work.join("direct-driver-build.log"),
        &temporary,
    );
    snapshot_artifacts(
        guard,
        "phase3-direct",
        "release",
        &[
            "cordis-direct-call-benchmark",
            &library("direct_benchmark_baseline"),
            &library("direct_benchmark_current"),
            &library("loader_fixture"),
        ],
    )
    .unwrap()
}

/// RM-7/8：Python依赖进入本次目录，Node构建复用现有SDK与KV实现。
fn script_dependencies(root: &Path, work: &Path, node: bool) -> PathBuf {
    let dependencies = match env::var_os("CORDIS_TEST_DEPENDENCIES") {
        Some(path) => PathBuf::from(path),
        None => {
            let path = work.join("dependencies");
            checked(
                &mut cordis_testkit::python::install_dependencies(
                    root,
                    &root.join("sdks/python/requirements.txt"),
                    &path,
                )
                .unwrap(),
                &work.join("python-dependencies.log"),
                &work.join("tmp"),
            );
            path
        }
    };
    if node && env::var_os("CORDIS_M21_SCRIPTS_READY").is_none() {
        for directory in ["sdks/typescript", "plugins/kv-ts"] {
            checked(
                Command::new("npm")
                    .current_dir(root.join(directory))
                    .env("npm_config_cache", root.join("target/npm-cache"))
                    .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"]),
                &work
                    .join(directory.replace('/', "-"))
                    .with_extension("install.log"),
                &work.join("tmp"),
            );
            let mut command = Command::new("npm");
            command
                .current_dir(root.join(directory))
                .env("npm_config_cache", root.join("target/npm-cache"))
                .args(["run", "build"]);
            if directory == "sdks/typescript" {
                command.args(["-w", "cordis-sdk"]);
            }
            checked(
                &mut command,
                &work
                    .join(directory.replace('/', "-"))
                    .with_extension("build.log"),
                &work.join("tmp"),
            );
        }
    }
    dependencies
}

fn main() {
    let arguments: Vec<_> = env::args().skip(1).collect();
    let mode = arguments.first().expect("需要rpc、replace或direct测量模式");
    assert!(matches!(
        mode.as_str(),
        "rpc" | "replace" | "direct" | "prepare"
    ));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap();
    let guard = TestGuard::new(&root, &format!("m21-{mode}"));
    let work = guard.directory("phase3-acceptance");
    fs::create_dir_all(work.join("tmp")).unwrap();
    if mode == "prepare" {
        prepare(&root, &guard, &work);
        return;
    }
    let tools = match env::var_os("CORDIS_M21_TOOLS") {
        Some(path) => PathBuf::from(path),
        None => programs(&root, &guard, &work, mode),
    };
    let mut command = Command::new("conda");
    command
        .current_dir(&root)
        .args(["run", "--no-capture-output", "-n", "base", "python"])
        .arg(root.join("benchmarks/multilanguage/measure.py"))
        .arg("--workspace")
        .arg(&root)
        .arg("--tools")
        .arg(&tools)
        .arg("--evidence")
        .arg(work.join("run"));
    if mode != "direct" {
        let dependencies = script_dependencies(&root, &work, mode == "rpc");
        command.arg("--dependencies").arg(dependencies);
    }
    command.args(&arguments);
    if mode == "rpc" {
        command
            .arg("--probe")
            .arg(tools.join(library("cordis_latency_probe")));
    }
    if mode == "direct" {
        let direct = env::var_os("CORDIS_M21_DIRECT_TOOLS")
            .map(PathBuf::from)
            .unwrap_or_else(|| direct_tools(&root, &guard, &work));
        command
            .arg("--driver")
            .arg(direct.join("cordis-direct-call-benchmark"))
            .arg("--provider")
            .arg(direct.join(library("loader_fixture")))
            .arg("--baseline")
            .arg(direct.join(library("direct_benchmark_baseline")))
            .arg("--current")
            .arg(direct.join(library("direct_benchmark_current")));
    }
    checked(
        &mut command,
        &work.join("measurement.log"),
        &work.join("tmp"),
    );
    eprintln!("测量证据：{}", guard.evidence_for(&work).display());
    guard.success();
}

/// RM-7/8/9：准备阶段保存运行副本与来源，计时阶段复用这些路径。
fn prepare(root: &Path, guard: &TestGuard, work: &Path) {
    let tools = programs(root, guard, work, "prepare");
    let direct = direct_tools(root, guard, work);
    let dependencies = script_dependencies(root, work, true);
    let providers = work.join("providers");
    checked(
        Command::new("conda")
            .current_dir(root)
            .args(["run", "--no-capture-output", "-n", "base", "python"])
            .arg(root.join("benchmarks/multilanguage/prepare_sources.py"))
            .arg("--workspace")
            .arg(root)
            .arg("--destination")
            .arg(&providers),
        &work.join("providers.log"),
        &work.join("tmp"),
    );
    let path = providers.join("typescript/cordis-plugin.toml");
    let mut manifest: toml_edit::DocumentMut = fs::read_to_string(&path).unwrap().parse().unwrap();
    manifest["restart"]["max"] = toml_edit::value(0);
    fs::write(path, manifest.to_string()).unwrap();
    let record = serde_json::json!({"state":"ready-for-measurement","profile":"release","tools":tools,"direct":direct,"dependencies":dependencies,"providers":providers,"work":work,"rm7_caller":"native Rust cdylib in its own cordis-host group","rm7_samples":10000,"rm8_samples_per_transition":1,"rm9_rounds":9,"rm9_warmup":10000,"rm9_iterations":100000});
    let output = root.join("target/phase3-acceptance/m21/prepared.json");
    fs::write(&output, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    eprintln!("测量产物准备完成：{}", output.display());
}
