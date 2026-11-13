//! DOC-15 M18、AU-18：生成的 Python 插件与真实核心、原生插件共同运行。

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[path = "support/script_tools.rs"]
mod script_tools;
use cordis_testkit::{cleanup::TestGuard, python::install_dependencies};
use script_tools::{Running, Tools, active, checked, tools};

#[test]
fn python_sdk_composes_migrates_and_passes_native_contracts() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "python-sdk-behavior");
    let tools = tools().for_case(&case);
    let evidence = case.track(
        tools
            .root
            .join("target/python-sdk")
            .join(format!("behavior-{}", std::process::id())),
    );
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
    eprintln!(
        "Python SDK 行为证据：{}",
        case.evidence_for(&evidence).display()
    );
    case.success();
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
        &mut install_dependencies(
            &tools.root,
            &plugin.join("requirements.txt"),
            &plugin.join(".dependencies"),
        )
        .unwrap(),
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
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "python-scaffold");
    let tools = tools().for_case(&case);
    let project = case.track(python_project(&tools));
    let mut core = start_core(&tools, &project);
    let before = active(&project, &mut core, None, "python-scaffold-new-version");
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
    let after = active(
        &project,
        &mut core,
        before["context"].as_u64(),
        "python-scaffold-new-version",
    );
    fs::write(
        project.join("accepted.json"),
        serde_json::to_vec_pretty(&serde_json::json!({"before": before, "after": after})).unwrap(),
    )
    .unwrap();
    checked(Command::new("/bin/kill").args(["-TERM", &core.0.id().to_string()]));
    assert!(core.0.wait().unwrap().success());
    case.success();
}
