//! DOC-15 M18、AU-18：生成的 Python 插件与真实核心、原生插件共同运行。

use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

struct Tools {
    root: PathBuf,
    binaries: PathBuf,
}

fn checked(command: &mut Command) {
    let result = command.output().expect("执行 Python 插件验收命令");
    assert!(
        result.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

/// Y-3、项目规则 11：统一构建并复制当前源码的运行程序、插件与管理客户端。
fn tools() -> &'static Tools {
    static TOOLS: OnceLock<Tools> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let _lock = cordis_testkit::build::shared_build_lock(&root).unwrap();
        let target = cordis_testkit::build::shared_target_dir(&root);
        checked(
            Command::new(env!("CARGO"))
                .current_dir(&root)
                .args([
                    "build",
                    "-p",
                    "cordis-runtime",
                    "-p",
                    "cordis-host",
                    "-p",
                    "cordis-dev",
                    "-p",
                    "kv-mem",
                    "-p",
                    "counter",
                    "-p",
                    "kv-contract",
                    "--target-dir",
                ])
                .arg(&target),
        );
        checked(
            Command::new(env!("CARGO"))
                .current_dir(&root)
                .args(["build", "--manifest-path"])
                .arg(root.join("crates/cordis-runtime/fixtures/plugin/Cargo.toml"))
                .arg("--target-dir")
                .arg(&target),
        );
        let libraries = ["kv_mem", "counter", "kv_contract"].map(|name| {
            format!(
                "{}{name}{}",
                env::consts::DLL_PREFIX,
                env::consts::DLL_SUFFIX
            )
        });
        let binaries = cordis_testkit::build::snapshot_artifacts(
            &root,
            "python-sdk-tools",
            "debug",
            &[
                "cordis",
                "cordis-dev",
                "cordis-host",
                "cordis-runtime-fixture",
                &libraries[0],
                &libraries[1],
                &libraries[2],
            ],
        )
        .unwrap();
        Tools { root, binaries }
    })
}

#[test]
fn python_sdk_composes_migrates_and_passes_native_contracts() {
    let tools = tools();
    let evidence = tools
        .root
        .join("target/python-sdk")
        .join(format!("behavior-{}", std::process::id()));
    checked(
        Command::new("conda")
            .current_dir(&tools.root)
            .args(["run", "--no-capture-output", "-n", "base", "python"])
            .arg(tools.root.join("tests/python-sdk/run.py"))
            .arg("--tools")
            .arg(&tools.binaries)
            .arg("--evidence")
            .arg(&evidence),
    );
    eprintln!("Python SDK 行为证据：{}", evidence.display());
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn active(project: &Path, core: &mut Running, previous: Option<u64>) -> Value {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            core.0.try_wait().unwrap().is_none(),
            "核心提前退出：{}",
            project.display()
        );
        if let Ok(bytes) = fs::read(project.join("state/status.json"))
            && let Ok(snapshot) = serde_json::from_slice::<Value>(&bytes)
            && let Some(instances) = snapshot["instances"].as_array()
            && instances.len() == 3
            && instances
                .iter()
                .all(|instance| instance["state"] == "active")
            && let Some(hello) = instances
                .iter()
                .find(|instance| instance["name"] == "hello")
            && previous.is_none_or(|old| hello["context"].as_u64() != Some(old))
            && (previous.is_none()
                || fs::read_to_string(project.join("core.log"))
                    .unwrap()
                    .contains("python-scaffold-new-version"))
        {
            return hello.clone();
        }
        assert!(
            Instant::now() < until,
            "等待当前 Python 版本激活：{}",
            project.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// DOC-15 §12.1：公开命令生成同时含 Rust 与 Python 插件的项目。
fn python_project(tools: &Tools) -> PathBuf {
    let project = tools
        .root
        .join("target/python-sdk")
        .join(format!("scaffold-{}", std::process::id()));
    checked(
        Command::new(tools.binaries.join("cordis-dev"))
            .arg("new")
            .arg(&project)
            .arg("--sdk-path")
            .arg(tools.root.join("crates/cordis-sdk")),
    );
    checked(
        Command::new(tools.binaries.join("cordis-dev"))
            .args(["new-plugin", "hello", "--python", "--project"])
            .arg(&project),
    );
    python_dependencies(tools, &project.join("plugins/hello"));
    checked(
        Command::new(env!("CARGO"))
            .current_dir(&project)
            .env_remove("CARGO_TARGET_DIR")
            .args(["build", "--target-dir", "target"]),
    );
    project
}

/// ADR-0040：依赖位于生成插件的目录，测试清单选择已安装的 Conda 解释器。
fn python_dependencies(tools: &Tools, plugin: &Path) {
    checked(
        Command::new("conda")
            .current_dir(&tools.root)
            .env("PIP_REQUIRE_VIRTUALENV", "false")
            .env("TMPDIR", tools.root.join("target/tmp"))
            .args([
                "run",
                "--no-capture-output",
                "-n",
                "base",
                "python",
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-deps",
                "--cache-dir",
            ])
            .arg(tools.root.join("target/python-sdk-dependencies/pip-cache"))
            .arg("--target")
            .arg(plugin.join(".dependencies"))
            .arg("-r")
            .arg(plugin.join("requirements.txt")),
    );
    let path = plugin.join("cordis-plugin.toml");
    let mut manifest: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    manifest["command"] = toml::Value::Array(
        [
            "conda",
            "run",
            "--no-capture-output",
            "-n",
            "base",
            "python",
            "-B",
            "main.py",
        ]
        .into_iter()
        .map(|value| value.into())
        .collect(),
    );
    fs::write(path, toml::to_string(&manifest).unwrap()).unwrap();
}

fn start_core(tools: &Tools, project: &Path) -> Running {
    let log = fs::File::create(project.join("core.log")).unwrap();
    let search = env::join_paths(
        std::iter::once(tools.binaries.clone())
            .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap();
    Running(
        Command::new(tools.binaries.join("cordis"))
            .arg("run")
            .current_dir(project)
            .env("PATH", search)
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    )
}

/// DOC-15 §12.1：新建 Python 插件后原 Rust 成员继续构建，watch 跟随源变化替换。
#[test]
fn python_scaffold_runs_and_watch_replaces_changed_source() {
    let tools = tools();
    let project = python_project(tools);
    let mut core = start_core(tools, &project);
    let before = active(&project, &mut core, None);
    let main = project.join("plugins/hello/main.py");
    let source = fs::read_to_string(&main).unwrap();
    fs::write(
        main,
        source.replace(
            "    await context.log",
            "    print(\"python-scaffold-new-version\", file=sys.stderr)\n    await context.log",
        ),
    )
    .unwrap();
    let after = active(&project, &mut core, before["context"].as_u64());
    fs::write(
        project.join("accepted.json"),
        serde_json::to_vec_pretty(&serde_json::json!({"before": before, "after": after})).unwrap(),
    )
    .unwrap();
    checked(Command::new("/bin/kill").args(["-TERM", &core.0.id().to_string()]));
    assert!(core.0.wait().unwrap().success());
    fs::remove_dir_all(project.join("target")).unwrap();
}
