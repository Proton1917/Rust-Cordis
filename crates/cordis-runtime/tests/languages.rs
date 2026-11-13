//! AU-3/4/5/6：固定语言组合、状态迁移与契约场景，编译及现场沿公共testkit管理。

use cordis_testkit::{
    build::{shared_build_lock, shared_target_dir, snapshot_artifacts},
    cleanup::{FixtureLease, SharedFixture, TestGuard},
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

/// M21：命令结果随现场保存，错误终止对应场景。
fn checked(command: &mut Command, log: &Path, temporary: &Path) {
    let output = command
        .env("TMPDIR", temporary)
        .output()
        .expect("执行跨语言验收命令");
    let mut text = output.stdout;
    text.extend(output.stderr);
    fs::write(log, &text).unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&text)
    );
}

/// 项目规则11：复用显式快照或一次构建，Weak缓存由实际场景租约控制寿命。
fn tools() -> FixtureLease<Tools> {
    static TOOLS: SharedFixture<Tools> = SharedFixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    TOOLS.get_or_init(&root, "phase3-language-tools", |guard| {
        let work = guard.directory("phase3-language-tools");
        fs::create_dir_all(work.join("tmp")).unwrap();
        let binaries = env::var_os("CORDIS_PHASE3_TOOLS")
            .map(PathBuf::from)
            .unwrap_or_else(|| build(&root, guard, &work));
        let dependencies = env::var_os("CORDIS_TEST_DEPENDENCIES")
            .map(PathBuf::from)
            .unwrap_or_else(|| python_dependencies(&root, &work));
        if env::var_os("CORDIS_PHASE3_SCRIPTS_READY").is_none() {
            script_builds(&root, &work);
        }
        Tools {
            root: root.clone(),
            binaries,
            dependencies,
        }
    })
}

/// 项目规则11：当前原生程序与管理夹具共用构建目录，复制产物时继续持锁。
fn build(root: &Path, guard: &TestGuard, work: &Path) -> PathBuf {
    let _lock = shared_build_lock(root).unwrap();
    let target = shared_target_dir(root);
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
                "kv-mem",
                "-p",
                "counter",
                "-p",
                "kv-contract",
                "--target-dir",
            ])
            .arg(&target),
        &work.join("programs-build.log"),
        &work.join("tmp"),
    );
    checked(
        Command::new(env!("CARGO"))
            .current_dir(root)
            .args(["build", "--manifest-path"])
            .arg(root.join("crates/cordis-runtime/fixtures/plugin/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target),
        &work.join("management-build.log"),
        &work.join("tmp"),
    );
    let libraries = ["kv_mem", "counter", "kv_contract"].map(|name| {
        format!(
            "{}{name}{}",
            env::consts::DLL_PREFIX,
            env::consts::DLL_SUFFIX
        )
    });
    snapshot_artifacts(
        guard,
        "phase3-language-tools",
        "debug",
        &[
            "cordis",
            "cordis-host",
            "cordis-runtime-fixture",
            &libraries[0],
            &libraries[1],
            &libraries[2],
        ],
    )
    .unwrap()
}

/// M18/M21：Python依赖进入夹具目录，安装输出由同一守卫保存。
fn python_dependencies(root: &Path, work: &Path) -> PathBuf {
    let destination = work.join("dependencies");
    checked(
        &mut cordis_testkit::python::install_dependencies(
            root,
            &root.join("sdks/python/requirements.txt"),
            &destination,
        )
        .unwrap(),
        &work.join("python-dependencies.log"),
        &work.join("tmp"),
    );
    destination
}

/// M18/M19：沿既有SDK与probe源码构建，使用同一npm缓存及公共构建锁。
fn script_builds(root: &Path, work: &Path) {
    let _lock = shared_build_lock(root).unwrap();
    for (label, directory) in [
        ("sdk", "sdks/typescript/cordis-sdk"),
        ("kv", "plugins/kv-ts"),
        ("probe", "tests/typescript-sdk/plugin"),
    ] {
        checked(
            Command::new("npm")
                .current_dir(root.join(directory))
                .env("npm_config_cache", root.join("target/npm-cache"))
                .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"]),
            &work.join(format!("{label}-install.log")),
            &work.join("tmp"),
        );
        checked(
            Command::new("npm")
                .current_dir(root.join(directory))
                .env("npm_config_cache", root.join("target/npm-cache"))
                .args(["run", "build"]),
            &work.join(format!("{label}-build.log")),
            &work.join("tmp"),
        );
    }
}

/// AU-3/4/5/6：守卫先于租约创建，真实核心和全部插件结束后归档证据。
fn scenario(name: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let case = TestGuard::new(&root, &format!("phase3-{name}"));
    let tools = tools().for_case(&case);
    let work = case.directory("phase3-acceptance");
    fs::create_dir_all(work.join("tmp")).unwrap();
    checked(
        Command::new("conda")
            .current_dir(&tools.root)
            .args(["run", "--no-capture-output", "-n", "base", "python"])
            .arg(tools.root.join("tests/phase3/run.py"))
            .arg("--workspace")
            .arg(&tools.root)
            .arg("--tools")
            .arg(&tools.binaries)
            .arg("--dependencies")
            .arg(&tools.dependencies)
            .arg("--evidence")
            .arg(work.join("case"))
            .arg("--scenario")
            .arg(name),
        &work.join("run.log"),
        &work.join("tmp"),
    );
    eprintln!("跨语言证据：{}", case.evidence_for(&work).display());
    case.success();
}

#[test]
fn three_languages_coexist_and_python_typescript_call_each_other() {
    scenario("cross");
}

#[test]
fn rust_counter_continues_across_rust_python_typescript_providers() {
    scenario("migration");
}
