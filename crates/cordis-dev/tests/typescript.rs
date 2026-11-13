//! DOC-15 M19、AU-18：TypeScript SDK 与原生插件共同执行服务、状态和组合语义。

#[path = "support/script_tools.rs"]
mod script_tools;
use cordis_testkit::cleanup::TestGuard;
use script_tools::{Running, Tools, active, checked, tools};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[test]
fn typescript_sdk_composes_migrates_and_passes_native_contracts() {
    let case = TestGuard::for_package(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "typescript-sdk-behavior",
    );
    let tools = tools().for_case(&case);
    let evidence = case.track(
        tools
            .root
            .join("target/typescript-sdk")
            .join(format!("behavior-{}", std::process::id())),
    );
    checked(
        Command::new("conda")
            .current_dir(&tools.root)
            .args(["run", "--no-capture-output", "-n", "base", "python"])
            .arg(tools.root.join("tests/typescript-sdk/run.py"))
            .arg("--tools")
            .arg(&tools.binaries)
            .arg("--evidence")
            .arg(&evidence),
    );
    eprintln!(
        "TypeScript SDK 行为证据：{}",
        case.evidence_for(&evidence).display()
    );
    case.success();
}

fn project(tools: &Tools) -> PathBuf {
    let project = tools
        .root
        .join("target/typescript-sdk")
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
            .args(["new-plugin", "hello", "--ts", "--project"])
            .arg(&project),
    );
    let plugin = project.join("plugins/hello");
    checked(
        Command::new("npm")
            .current_dir(&tools.root)
            .args([
                "install",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                "--cache",
            ])
            .arg(tools.root.join("target/npm-cache"))
            .arg("--prefix")
            .arg(&plugin),
    );
    build_plugin(&plugin);
    checked(
        Command::new(env!("CARGO"))
            .current_dir(&project)
            .env_remove("CARGO_TARGET_DIR")
            .args(["build", "--target-dir", "target"]),
    );
    project
}

fn build_plugin(plugin: &Path) {
    checked(
        Command::new("npm")
            .args(["run", "build", "--prefix"])
            .arg(plugin),
    );
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

/// DOC-15 §12.1：生成项目编译普通 Node 文件，watch 取得新模块后执行其实际输出。
#[test]
fn typescript_scaffold_runs_and_watch_replaces_changed_module() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "typescript-scaffold");
    let tools = tools().for_case(&case);
    let project = case.track(project(&tools));
    let mut core = start_core(&tools, &project);
    let marker = "typescript-scaffold-new-version";
    let before = active(&project, &mut core, None, marker);
    let plugin = project.join("plugins/hello");
    let main = plugin.join("src/main.ts");
    let source = fs::read_to_string(&main).unwrap();
    fs::write(main, source.replace("        await context.log", "        console.error(\"typescript-scaffold-new-version\");\n        await context.log")).unwrap();
    build_plugin(&plugin);
    let after = active(&project, &mut core, before["context"].as_u64(), marker);
    fs::write(
        project.join("accepted.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"before":before,"after":after,"marker":marker}),
        )
        .unwrap(),
    )
    .unwrap();
    checked(Command::new("/bin/kill").args(["-TERM", &core.0.id().to_string()]));
    assert!(core.0.wait().unwrap().success());
    case.success();
}
