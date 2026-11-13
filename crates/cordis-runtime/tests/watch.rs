//! DOC-15 §3.3、DOC-14 M12：真实文件事件驱动安装、替换、失败回滚与配置重读。

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Core(Option<Child>);

impl Core {
    fn start(project: &Path, runtime: &Path) -> Self {
        let log = fs::File::create(project.join("core.log")).unwrap();
        let child = Command::new(runtime)
            .arg("run")
            .current_dir(project)
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        Self(Some(child))
    }

    fn finish(&mut self) {
        let child = self.0.as_mut().unwrap();
        assert!(
            Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "核心退出：{status}");
                self.0 = None;
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "核心有序退出超时"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _signal = child.kill();
            let _reaped = child.wait();
        }
    }
}

fn build(mut command: Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn library(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!(
        "{}{name}.{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_EXTENSION
    ))
}

fn host_binary(workspace: &Path) -> PathBuf {
    workspace
        .join("target/watch-tools")
        .join(std::process::id().to_string())
        .join("cordis-host")
}

fn prepare(workspace: &Path) -> (PathBuf, PathBuf) {
    let fixtures = workspace
        .join("target/host-native")
        .join(std::process::id().to_string());
    compile_host_fixtures(workspace, &fixtures);
    compile_watch(workspace, &fixtures);
    let project = create_project(workspace, &fixtures, "window");
    (project, fixtures)
}

fn compile_host_fixtures(workspace: &Path, fixtures: &Path) {
    let shared = cordis_testkit::build::shared_target_dir(workspace);
    let build_lock = cordis_testkit::build::shared_build_lock(workspace).unwrap();
    let mut compile = Command::new(env!("CARGO"));
    compile
        .args(["build", "-p", "cordis-host", "--target-dir"])
        .arg(&shared)
        .current_dir(workspace);
    build(compile);
    fs::create_dir_all(host_binary(workspace).parent().unwrap()).unwrap();
    fs::copy(shared.join("debug/cordis-host"), host_binary(workspace)).unwrap();
    drop(build_lock);
    let mut fixtures_command = Command::new(env!("CARGO"));
    fixtures_command
        .args(["run", "--manifest-path"])
        .arg(workspace.join("crates/cordis-host/tests/driver/Cargo.toml"))
        .arg("--target-dir")
        .arg(&shared)
        .args(["--", "--build-only", "--artifacts-dir"])
        .arg(fixtures)
        .arg("--host")
        .arg(host_binary(workspace));
    build(fixtures_command);
    let _build = cordis_testkit::build::shared_build_lock(workspace).unwrap();
    let mut current_consumer = Command::new(env!("CARGO"));
    current_consumer
        .args(["build", "--manifest-path"])
        .arg(workspace.join("crates/cordis-host/tests/fixtures/Cargo.toml"))
        .args(["-p", "loader-fixture-consumer", "--target-dir"])
        .arg(&shared);
    build(current_consumer);
    let copied = fixtures.join("fixtures-current/debug");
    fs::create_dir_all(&copied).unwrap();
    fs::copy(
        library(&shared.join("debug"), "loader_fixture_consumer"),
        library(&copied, "loader_fixture_consumer"),
    )
    .unwrap();
}

fn compile_watch(workspace: &Path, fixtures: &Path) {
    let _build = cordis_testkit::build::shared_build_lock(workspace).unwrap();
    let shared = cordis_testkit::build::shared_target_dir(workspace);
    let watch = workspace.join("plugins/watch");
    let watch_target = workspace
        .join("target/watch-plugin")
        .join(std::process::id().to_string());
    let mut watcher = Command::new(env!("CARGO"));
    watcher
        .args(["build", "--manifest-path"])
        .arg(watch.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&shared);
    build(watcher);
    fs::create_dir_all(watch_target.join("debug")).unwrap();
    fs::copy(
        library(&shared.join("debug"), "cordis_watch"),
        library(&watch_target.join("debug"), "cordis_watch"),
    )
    .unwrap();
    let mut failed = Command::new(env!("CARGO"));
    failed
        .args(["build", "--manifest-path"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/watch-fails/Cargo.toml"))
        .arg("--target-dir")
        .arg(&shared);
    build(failed);
    let copied = fixtures.join("watch-fails/debug");
    fs::create_dir_all(&copied).unwrap();
    fs::copy(
        library(&shared.join("debug"), "watch_fails"),
        library(&copied, "watch_fails"),
    )
    .unwrap();
}

fn create_project(workspace: &Path, fixtures: &Path, label: &str) -> PathBuf {
    let project = workspace
        .join("target/watch-runtime")
        .join(format!("{label}-{}", std::process::id()));
    fs::create_dir_all(project.join("plugins")).unwrap();
    fs::create_dir_all(project.join("sources")).unwrap();
    let watch_target = workspace
        .join("target/watch-plugin")
        .join(std::process::id().to_string());
    fs::copy(env!("CARGO_BIN_EXE_cordis"), project.join("cordis")).unwrap();
    fs::copy(host_binary(workspace), project.join("cordis-host")).unwrap();
    fs::copy(
        library(&watch_target.join("debug"), "cordis_watch"),
        library(&project.join("plugins"), "watch"),
    )
    .unwrap();
    fs::copy(
        fixtures
            .join("variants")
            .join(format!("provider-v1.{}", std::env::consts::DLL_EXTENSION)),
        library(&project.join("plugins"), "provider"),
    )
    .unwrap();
    fs::copy(
        library(
            &fixtures.join("fixtures-185/debug"),
            "loader_fixture_consumer",
        ),
        library(&project.join("plugins"), "consumer"),
    )
    .unwrap();
    project
}

fn configuration(added: bool) -> String {
    let mut contents = String::from(
        r#"requires_core = "^0.1"
[runtime]
confirm_ms = 50
checkpoint_ms = 100
restart_max = 0
[instance.provider]
plugin = "loader-fixture"
component = "provider"
group = "application"
config = { initial = 137 }
[instance.reader]
plugin = "loader-consumer"
component = "low"
group = "application"
[instance.idle]
plugin = "loader-consumer"
component = "idle"
group = "independent"
[instance.watch]
plugin = "watch"
component = "watch"
grants = { "cordis.manage@1" = ["install", "replace_batch", "reload"] }
config = { paths = ["sources"], debounce_ms = 300 }
"#,
    );
    if added {
        contents.push_str("\n[instance.added]\nplugin = 'loader-consumer'\ncomponent = 'idle'\ngroup = 'independent'\n");
    }
    contents
}

fn status(project: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(project.join("state/status.json")).ok()?).ok()
}

fn instance<'a>(snapshot: &'a Value, name: &str) -> Option<&'a Value> {
    snapshot["instances"]
        .as_array()?
        .iter()
        .find(|instance| instance["name"] == name)
}

fn wait(project: &Path, reason: &str, accept: impl Fn(&Value) -> bool) -> Value {
    let started = Instant::now();
    loop {
        if let Some(snapshot) = status(project).filter(|snapshot| accept(snapshot)) {
            return snapshot;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "等待 {reason} 超时；日志 {}",
            project.join("core.log").display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn swap(snapshot: &Value) -> Option<&Value> {
    snapshot["last_swap"]
        .as_array()?
        .iter()
        .find(|report| report["plugin"] == "loader-fixture")
}

fn publish(source: &Path, destination: &Path) {
    let staged = destination.with_extension("pending");
    fs::copy(source, &staged).unwrap();
    fs::rename(staged, destination).unwrap();
}

fn consumers_built_with(snapshot: &Value, version: &str) -> bool {
    let active: Vec<_> = snapshot["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|library| library["name"] == "loader-consumer" && library["retired"] == false)
        .collect();
    !active.is_empty()
        && active.iter().all(|library| {
            library["build_info"]
                .as_str()
                .is_some_and(|build| build.contains(version))
        })
}

#[test]
fn watch_replaces_rolls_back_suppresses_same_candidate_and_reloads() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let (project, artifacts) = prepare(workspace);
    fs::write(project.join("cordis.toml"), configuration(false)).unwrap();
    let mut core = Core::start(&project, &project.join("cordis"));
    let initial = wait(&project, "全部初始实例加载", |snapshot| {
        ["provider", "reader", "idle", "watch"].iter().all(|name| {
            instance(snapshot, name).is_some_and(|instance| instance["state"] == "active")
        })
    });
    let idle_revision = instance(&initial, "idle").unwrap()["revision"].clone();
    let source = library(&project.join("sources"), "provider");
    publish(
        &artifacts
            .join("variants")
            .join(format!("provider-v2.{}", std::env::consts::DLL_EXTENSION)),
        &source,
    );
    let replaced = wait(&project, "watch完成组内换库", |snapshot| {
        swap(snapshot).is_some_and(|report| report["succeeded"] == true)
    });
    assert_eq!(
        instance(&replaced, "idle").unwrap()["revision"],
        idle_revision
    );
    let accepted = swap(&replaced).unwrap()["candidate_library"].clone();
    let failed = artifacts.join("variants").join(format!(
        "provider-fails.{}",
        std::env::consts::DLL_EXTENSION
    ));
    let consumer_source = library(&project.join("sources"), "consumer");
    let provider_staged = source.with_extension("pending");
    let consumer_staged = consumer_source.with_extension("pending");
    let configuration_staged = project.join("cordis.next");
    fs::copy(&failed, &provider_staged).unwrap();
    fs::copy(
        library(
            &artifacts.join("fixtures-current/debug"),
            "loader_fixture_consumer",
        ),
        &consumer_staged,
    )
    .unwrap();
    fs::write(&configuration_staged, configuration(true)).unwrap();
    fs::rename(provider_staged, &source).unwrap();
    fs::rename(consumer_staged, &consumer_source).unwrap();
    fs::rename(configuration_staged, project.join("cordis.toml")).unwrap();
    let rolled_back = wait(&project, "watch失败候选回滚", |snapshot| {
        swap(snapshot).is_some_and(|report| {
            report["rolled_back"] == true && report["candidate_library"] != accepted
        })
    });
    let rejected = swap(&rolled_back).unwrap()["candidate_library"].clone();
    let restored = wait(&project, "整批原配置和版本恢复", |snapshot| {
        ["provider", "reader"].iter().all(|name| {
            instance(snapshot, name).is_some_and(|instance| instance["state"] == "active")
        }) && instance(snapshot, "added").is_none()
            && consumers_built_with(snapshot, "1.85.0")
    });
    assert_eq!(
        instance(&restored, "watch").unwrap()["revision"],
        instance(&initial, "watch").unwrap()["revision"]
    );
    publish(&failed, &source);
    thread::sleep(Duration::from_secs(1));
    assert_eq!(
        swap(&status(&project).unwrap()).unwrap()["candidate_library"],
        rejected,
        "相同被拒副本保持当前版本"
    );
    publish(
        &artifacts
            .join("variants")
            .join(format!("provider-v2.{}", std::env::consts::DLL_EXTENSION)),
        &source,
    );
    wait(
        &project,
        "修复一项后整批候选和配置共同提交",
        |snapshot| {
            instance(snapshot, "added").is_some_and(|instance| instance["state"] == "active")
                && consumers_built_with(snapshot, "1.96.0")
        },
    );
    core.finish();
    self_replacement(workspace, &artifacts);
}

/// ADR-0031：自身失败会清理原任务，新实例根据待确认窗口和核心结果抑制同内容重试。
fn self_replacement(workspace: &Path, artifacts: &Path) {
    let project = create_project(workspace, artifacts, "self");
    let mut configuration: toml::Value = toml::from_str(&configuration(false)).unwrap();
    configuration["instance"]["watch"]["config"]["paths"] =
        toml::Value::Array(vec![toml::Value::String("plugins".into())]);
    fs::write(
        project.join("cordis.toml"),
        toml::to_string(&configuration).unwrap(),
    )
    .unwrap();
    let mut core = Core::start(&project, &project.join("cordis"));
    wait(&project, "watch初始加载", |snapshot| {
        instance(snapshot, "watch").is_some_and(|instance| instance["state"] == "active")
    });
    let failed = library(&artifacts.join("watch-fails/debug"), "watch_fails");
    let source = library(&project.join("plugins"), "watch");
    publish(&failed, &source);
    let rejected = wait(&project, "自身失败批次实际回滚", |snapshot| {
        instance(snapshot, "watch").is_some_and(|instance| instance["state"] == "active")
            && snapshot["last_batch"].as_array().is_some_and(|outcomes| {
                outcomes
                    .iter()
                    .any(|outcome| outcome["caller"] == "watch" && outcome["state"] == "rejected")
            })
    });
    let outcome = rejected["last_batch"]
        .as_array()
        .unwrap()
        .iter()
        .find(|outcome| outcome["caller"] == "watch")
        .unwrap();
    let id = outcome["id"].clone();
    publish(&failed, &source);
    thread::sleep(Duration::from_secs(1));
    let observed = status(&project).unwrap();
    assert_eq!(
        observed["last_batch"]
            .as_array()
            .unwrap()
            .iter()
            .find(|outcome| outcome["caller"] == "watch")
            .unwrap()["id"],
        id,
        "自身回滚后相同内容保持当前版本"
    );
    core.finish();
    let mut restarted = Core::start(&project, &project.join("cordis"));
    wait(
        &project,
        "核心重启后继续使用已恢复的watch版本",
        |snapshot| {
            instance(snapshot, "watch").is_some_and(|instance| instance["state"] == "active")
        },
    );
    restarted.finish();
}
