//! DOC-15 §12.1、AU-8：经核心转交生成项目，真实编译并启动独立插件宿主。

use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn run(command: &mut Command) {
    let output = command.output().expect("运行验收命令");
    assert!(
        output.status.success(),
        "命令失败 {command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn binaries(root: &Path) -> (PathBuf, PathBuf) {
    if let (Some(core), Some(host)) = (
        env::var_os("CORDIS_CORE_BIN"),
        env::var_os("CORDIS_HOST_BIN"),
    ) {
        return (core.into(), host.into());
    }
    let _build = cordis_testkit::build::shared_build_lock(root).unwrap();
    let build = cordis_testkit::build::shared_target_dir(root);
    run(Command::new("cargo")
        .current_dir(root)
        .args([
            "build",
            "-p",
            "cordis-runtime",
            "-p",
            "cordis-host",
            "--target-dir",
        ])
        .arg(&build));
    let copied = cordis_testkit::build::snapshot_artifacts(
        root,
        "scaffold-tools",
        "debug",
        &["cordis", "cordis-host"],
    )
    .unwrap();
    (copied.join("cordis"), copied.join("cordis-host"))
}

/// 项目须在 30 秒内报告三个实例 active；失败保留项目、状态和日志供定位。
fn wait_active(project: &Path, running: &mut Running) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = fs::read(project.join("state/status.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        if let Some(snapshot) = snapshot
            && let Some(instances) = snapshot["instances"].as_array()
            && ["app", "watch", "hello"].iter().all(|name| {
                instances
                    .iter()
                    .any(|instance| instance["name"] == *name && instance["state"] == "active")
            })
        {
            return snapshot;
        }
        assert!(
            running.0.try_wait().unwrap().is_none(),
            "核心提前退出：{}",
            project.display()
        );
        assert!(
            Instant::now() < deadline,
            "等待实例 active 超时：{}",
            project.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn forwarded_scaffold_builds_and_runs_native_plugins() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let (core, host) = binaries(&root);
    let dev = Path::new(env!("CARGO_BIN_EXE_cordis-dev"));
    let mut directories = vec![
        dev.parent().unwrap().to_owned(),
        host.parent().unwrap().to_owned(),
    ];
    directories.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let path = env::join_paths(directories).unwrap();
    let project = root.join("target/dev-scaffold").join(format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&project).unwrap();
    run(Command::new(&core)
        .env("PATH", &path)
        .arg("new")
        .arg(&project)
        .arg("--sdk-path")
        .arg(root.join("crates/cordis-sdk")));
    run(Command::new(&core)
        .env("PATH", &path)
        .args(["new-plugin", "hello", "--rust", "--project"])
        .arg(&project));
    run(Command::new("cargo")
        .current_dir(&project)
        .args(["build", "--target-dir", "target"]));
    let output = fs::File::create(project.join("runtime.stdout.log")).unwrap();
    let errors = fs::File::create(project.join("runtime.stderr.log")).unwrap();
    let mut running = Running(
        Command::new(&core)
            .env("PATH", &path)
            .current_dir(&project)
            .arg("run")
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(errors)
            .spawn()
            .unwrap(),
    );
    let snapshot = wait_active(&project, &mut running);
    assert_eq!(
        snapshot["instances"].as_array().unwrap().len(),
        3,
        "生成项目的三个实例均由核心登记"
    );
    run(Command::new("/bin/kill").args(["-TERM", &running.0.id().to_string()]));
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = running.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "核心有序退出超时：{}",
            project.display()
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "核心有序退出失败：{}", project.display());
    fs::remove_dir_all(project.join("target")).unwrap();
    eprintln!("脚手架运行证据：{}", project.display());
}
