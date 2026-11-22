use cordis_runtime::storage;
use cordis_testkit::cleanup::{FixtureLease, SharedFixture, TestGuard};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[path = "runtime/gate.rs"]
mod gate;

#[path = "runtime/migration.rs"]
mod migration;

#[path = "runtime/management.rs"]
mod management;

#[path = "runtime/isolation.rs"]
mod isolation;

#[path = "runtime/confirmation.rs"]
mod confirmation;

#[path = "runtime/offline.rs"]
mod offline;

struct Process(Option<Child>);
impl Process {
    fn start(project: &Path, label: &str) -> Self {
        Self::start_binary(Path::new(env!("CARGO_BIN_EXE_cordis")), project, label)
    }

    fn start_binary(binary: &Path, project: &Path, label: &str) -> Self {
        let log = fs::File::create(project.join(format!("core-{label}.log"))).unwrap();
        let child = Command::new(binary)
            .args(["run", "--config"])
            .arg(project.join("cordis.toml"))
            .current_dir(project)
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let expected_pid = child.id();
        let mut process = Self(Some(child));
        until("core publishes its process identity", || {
            if let Some(exit) = process.0.as_mut().unwrap().try_wait().unwrap() {
                panic!(
                    "core exited during startup: {exit}; logs: {}",
                    project.display()
                );
            }
            status(project).filter(|snapshot| snapshot["pid"] == expected_pid)
        });
        process
    }
    fn kill(&mut self) {
        let mut child = self.0.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
    }
    fn finish(&mut self) {
        let child = self.0.as_mut().unwrap();
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "core exit: {status}");
                self.0 = None;
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "core shutdown deadline"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _exited = child.kill();
            let _reaped = child.wait();
        }
    }
}

fn fixture() -> FixtureLease<PathBuf> {
    static FIXTURE: SharedFixture<PathBuf> = SharedFixture::new();
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = package.parent().and_then(Path::parent).unwrap();
    FIXTURE.get_or_init(workspace, "runtime-fixture", |guard| {
        let _build = cordis_testkit::build::shared_build_lock(workspace).unwrap();
        let target = cordis_testkit::build::shared_target_dir(workspace);
        let output = Command::new(env!("CARGO"))
            .args(["build", "--manifest-path"])
            .arg(package.join("fixtures/plugin/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        cordis_testkit::build::snapshot_artifacts(
            guard,
            "runtime-fixture",
            "debug",
            &["cordis-runtime-fixture"],
        )
        .unwrap()
        .join("cordis-runtime-fixture")
    })
}

fn project(case: &TestGuard, name: &str) -> PathBuf {
    let project = case.directory(&format!("runtime-m12/cases/{name}"));
    fs::create_dir_all(project.join("plugins")).unwrap();
    fs::create_dir_all(project.join("sources")).unwrap();
    project
}

fn plugin(directory: &Path, role: &str, mode: &str) {
    fs::create_dir_all(directory).unwrap();
    let version = if matches!(mode, "v2" | "next") {
        "0.2.0"
    } else {
        "0.1.0"
    };
    let manifest = format!(
        "name = 'fixture-{role}'\nversion = '{version}'\nsocket = 'process'\nprotocol = 1\ncommand = [{}, '{role}', '{mode}']\nrestart = {{ max = 0, backoff_ms = 500 }}\n",
        toml::Value::String(fixture().to_string_lossy().into_owned())
    );
    fs::write(directory.join("cordis-plugin.toml"), manifest).unwrap();
}

fn quoted(path: &Path) -> String {
    toml::Value::String(path.to_string_lossy().into_owned()).to_string()
}

fn until<T>(description: &str, mut operation: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(value) = operation() {
            return value;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "deadline: {description}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn status(project: &Path) -> Option<Value> {
    fs::read(project.join("state/status.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn instance<'a>(snapshot: &'a Value, name: &str) -> Option<&'a Value> {
    snapshot["instances"]
        .as_array()?
        .iter()
        .find(|instance| instance["name"] == name)
}

fn active(project: &Path, name: &str) -> Value {
    until("instance active", || {
        let snapshot = status(project)?;
        (instance(&snapshot, name)?["state"] == "active").then_some(snapshot)
    })
}

fn send(project: &Path, id: u64, interface: &str, method: &str, args: Value) -> Value {
    let (commands, results) =
        if interface.starts_with("fixture.counter@") || interface.starts_with("kv@") {
            ("client-commands", "client-results")
        } else {
            ("commands", "results")
        };
    let command = project.join(commands).join(format!("{id:03}.json"));
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    let staged = command.with_extension("next");
    fs::write(
        &staged,
        serde_json::to_vec(&json!({ "interface": interface, "method": method, "args": args }))
            .unwrap(),
    )
    .unwrap();
    fs::rename(staged, command).unwrap();
    until("management reply", || {
        fs::read(project.join(results).join(format!("{id:03}.json")))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    })
}

fn manage(project: &Path, id: u64, method: &str, args: Value) -> Value {
    let reply = send(project, id, "cordis.manage@2.0", method, args);
    if matches!(method, "install" | "shutdown") || reply["ok"] != true {
        return reply;
    }
    let request_id = reply["value"]["id"].as_str().unwrap();
    let outcome = until("batch reaches a final result", || {
        let snapshot = status(project)?;
        snapshot["last_batch"]
            .as_array()?
            .iter()
            .find(|outcome| {
                outcome["caller"] == "admin"
                    && outcome["id"] == request_id
                    && outcome["state"] != "pending"
            })
            .cloned()
    });
    json!({ "ok": matches!(outcome["state"].as_str(), Some("succeeded" | "committed_with_cleanup")),
        "state": outcome["state"], "code": outcome["code"], "value": outcome["error"] })
}

fn version(project: &Path, expected: &str) {
    until("plugin version", || {
        let snapshot = status(project)?;
        let group = snapshot["groups"]
            .as_array()?
            .iter()
            .find(|group| group["name"] == "fixture-store")?;
        group["plugins"]
            .as_array()?
            .iter()
            .any(|plugin| plugin["name"] == "fixture-store" && plugin["version"] == expected)
            .then_some(())
    });
}

fn checkpoint(project: &Path, expected: u64) {
    until("checkpoint value", || {
        let bytes = fs::read(project.join("state/store.state")).ok()?;
        let state = storage::parse_state(&bytes).ok()?;
        state
            .data
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
            .filter(|value| *value == expected)
    });
}

// M12：真实管理调用验证权限、覆盖层、替换回滚、核心崩溃恢复与有序退出。
#[test]
fn management_and_checkpoints_survive_core_restart() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "management");
    let _fixture = fixture().for_case(&case);
    let project = project(&case, "management");
    plugin(&project.join("plugins/store"), "store", "v1");
    plugin(&project.join("plugins/admin"), "admin", "v1");
    plugin(&project.join("plugins/client"), "client", "v1");
    plugin(&project.join("sources/store-v2"), "store", "v2");
    let mut configuration = format!(
        "requires_core = '^0.1'\n[runtime]\ncheckpoint_ms = 100\nconfirm_ms = 20\nlifecycle_timeout_ms = 5000\n\n[instance.store]\nplugin = 'store'\nconfig = {{ initial = 41, writes = 5 }}\n\n[instance.admin]\nplugin = 'admin'\ngrants = {{ 'cordis.manage@1' = ['install', 'replace', 'rollback', 'configure', 'reload', 'shutdown'] }}\nconfig = {{ commands = {}, results = {} }}\n\n[instance.denied]\nplugin = 'admin'\nconfig = {{ commands = {}, results = {} }}\n",
        quoted(&project.join("commands")),
        quoted(&project.join("results")),
        quoted(&project.join("denied-commands")),
        quoted(&project.join("denied-results"))
    );
    configuration.push_str(&client_configuration(&project));
    fs::write(project.join("cordis.toml"), &configuration).unwrap();
    if cfg!(target_os = "macos") {
        let service = project.join("service/fixture.plist");
        let installed = Command::new(env!("CARGO_BIN_EXE_cordis"))
            .args(["service", "install", "--dry-run", "--config"])
            .arg(project.join("cordis.toml"))
            .arg("--output")
            .arg(&service)
            .output()
            .unwrap();
        assert!(
            installed.status.success(),
            "{}",
            String::from_utf8_lossy(&installed.stderr)
        );
        let accepted = Command::new("plutil")
            .arg("-lint")
            .arg(service)
            .output()
            .unwrap();
        assert!(
            accepted.status.success(),
            "{}",
            String::from_utf8_lossy(&accepted.stdout)
        );
    }
    let mut core = Process::start(&project, "first");
    active(&project, "admin");
    checkpoint(&project, 46);
    until("ungranted instance pending", || {
        let snapshot = status(&project)?;
        (instance(&snapshot, "denied")?["state"] == "pending").then_some(())
    });
    let denied = manage(&project, 1, "add", json!(["other", "store", null, {}]));
    assert_eq!(denied["code"], 9, "{denied}");
    let runtime = send(&project, 2, "cordis.runtime@1.1", "snapshot", json!([]));
    assert_eq!(runtime["ok"], true, "{runtime}");
    assert!(
        runtime["value"]["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["name"] == "fixture-store")
    );
    let old_context = instance(&active(&project, "admin"), "admin").unwrap()["context"].clone();
    let configured = manage(
        &project,
        3,
        "configure",
        json!(["admin", { "commands": project.join("commands"), "results": project.join("results"), "initial": 7 }]),
    );
    assert_eq!(configured["ok"], true, "{configured}");
    until("self configure completes after response", || {
        let snapshot = status(&project)?;
        let current = instance(&snapshot, "admin")?;
        (current["state"] == "active" && current["context"] != old_context).then_some(())
    });
    let before_invalid = instance(&active(&project, "store"), "store").unwrap()["revision"].clone();
    fs::write(project.join("cordis.toml"), "[invalid").unwrap();
    let invalid = manage(&project, 4, "reload", json!([]));
    assert_eq!(invalid["code"], 2, "{invalid}");
    assert_eq!(
        instance(&active(&project, "store"), "store").unwrap()["revision"],
        before_invalid
    );
    fs::write(project.join("cordis.toml"), &configuration).unwrap();
    let installed = manage(
        &project,
        5,
        "install",
        json!([project.join("sources/store-v2")]),
    );
    assert_eq!(installed["ok"], true, "{installed}");
    let reference = installed["value"].as_str().unwrap().to_owned();
    let duplicate = manage(
        &project,
        6,
        "install",
        json!([project.join("sources/store-v2")]),
    );
    assert_eq!(duplicate["value"], reference);
    assert_eq!(
        manage(&project, 7, "replace", json!(["store", reference]))["ok"],
        true
    );
    version(&project, "0.2.0");
    assert_eq!(
        send(&project, 70, "fixture.counter@1.0", "set", json!([91]))["ok"],
        true
    );
    checkpoint(&project, 91);
    let older_checkpoint = fs::read(project.join("state/store.state")).unwrap();
    plugin(&project.join("plugins/store"), "store", "fail");
    assert_eq!(
        manage(&project, 8, "rollback", json!(["store"]))["ok"],
        true
    );
    version(&project, "0.1.0");
    // 回滚换回上一版本的代码，状态继承当前值。
    checkpoint(&project, 91);
    assert_eq!(
        manage(&project, 9, "replace", json!(["store", reference]))["ok"],
        true
    );
    version(&project, "0.2.0");
    assert_eq!(
        send(&project, 71, "fixture.counter@1.0", "set", json!([137]))["ok"],
        true
    );
    checkpoint(&project, 137);
    let snapshot = status(&project).unwrap();
    let children = snapshot["groups"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|group| group["pid"].as_u64())
        .collect::<Vec<_>>();
    core.kill();
    for pid in children {
        until("plugin EOF exits after core SIGKILL", || {
            let alive = Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            (!alive).then_some(())
        });
    }
    // 升级时根目录保存了 137，旧版挂载留下的副本仍是 91。
    let legacy = project.join("state/consumed/store.state");
    fs::write(&legacy, older_checkpoint).unwrap();
    let mut core = Process::start(&project, "restarted");
    active(&project, "admin");
    version(&project, "0.2.0");
    assert_eq!(
        send(&project, 73, "fixture.counter@1.0", "get", json!([]))["value"],
        137
    );
    assert!(!legacy.exists());
    let status_output = Command::new(env!("CARGO_BIN_EXE_cordis"))
        .args(["status", "--config"])
        .arg(project.join("cordis.toml"))
        .output()
        .unwrap();
    assert!(
        status_output.status.success(),
        "{}",
        String::from_utf8_lossy(&status_output.stderr)
    );
    let status: Value = serde_json::from_slice(&status_output.stdout).unwrap();
    assert!(
        status["overrides"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "store")
    );
    assert_eq!(
        manage(&project, 72, "rollback", json!(["store"]))["ok"],
        true
    );
    version(&project, "0.1.0");
    checkpoint(&project, 137);
    let shutdown = manage(&project, 10, "shutdown", json!([]));
    assert_eq!(shutdown["ok"], true, "{shutdown}");
    core.finish();
    let persisted =
        storage::parse_state(&fs::read(project.join("state/store.state")).unwrap()).unwrap();
    assert_eq!(
        u64::try_from(persisted.data.as_integer().unwrap()).unwrap(),
        137
    );
    case.success();
}

/// DOC-11 §7：回滚换回上一版本并跨核心重启保持；无状态的旧版本也能正常接手。
#[test]
fn rollback_returns_to_previous_version_across_restart() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "empty-rollback");
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "empty-rollback", "stateless");
    let mut core = Process::start(&project, "empty-first");
    active(&project, "admin");
    let installed = manage(
        &project,
        1,
        "install",
        json!([project.join("sources/store-v2")]),
    );
    assert_eq!(installed["ok"], true, "{installed}");
    let replaced = manage(&project, 2, "replace", json!(["store", installed["value"]]));
    assert_eq!(replaced["ok"], true, "{replaced}");
    let written = send(&project, 3, "fixture.counter@1.0", "set", json!([99]));
    assert_eq!(written["ok"], true, "{written}");
    checkpoint(&project, 99);
    let rolled_back = manage(&project, 4, "rollback", json!(["store"]));
    assert_eq!(rolled_back["ok"], true, "{rolled_back}");
    version(&project, "0.1.0");
    let answered = send(&project, 5, "fixture.counter@1.0", "get", json!([]));
    assert_eq!(answered["ok"], true, "{answered}");
    assert_eq!(manage(&project, 6, "shutdown", json!([]))["ok"], true);
    core.finish();
    let mut core = Process::start(&project, "empty-restarted");
    active(&project, "admin");
    version(&project, "0.1.0");
    let answered = send(&project, 7, "fixture.counter@1.0", "get", json!([]));
    assert_eq!(answered["ok"], true, "{answered}");
    assert_eq!(manage(&project, 8, "shutdown", json!([]))["ok"], true);
    core.finish();
    case.success();
}

/// DOC-11 SW-9：旧进程拒绝退役时，已提交的新版本跨核心重启继续生效。
#[test]
fn replace_keeps_new_source_after_committed_retirement_failure() {
    let case = TestGuard::for_package(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "committed-retirement",
    );
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "committed-retirement", "shutdown-fail");
    let mut core = Process::start(&project, "retirement-first");
    active(&project, "admin");
    let written = send(&project, 1, "fixture.counter@1.0", "set", json!([63]));
    assert_eq!(written["ok"], true, "{written}");
    let installed = manage(
        &project,
        2,
        "install",
        json!([project.join("sources/store-v2")]),
    );
    assert_eq!(installed["ok"], true, "{installed}");
    let replaced = manage(&project, 3, "replace", json!(["store", installed["value"]]));
    assert_eq!(replaced["ok"], true, "{replaced}");
    version(&project, "0.2.0");
    let overlay = storage::Store::create(project.join("state"))
        .unwrap()
        .overlay()
        .unwrap();
    assert_eq!(
        overlay
            .instance
            .get("store")
            .map(|entry| entry.plugin.as_str()),
        installed["value"].as_str()
    );
    let restored = send(&project, 4, "fixture.counter@1.0", "get", json!([]));
    assert_eq!(restored["value"], 63, "{restored}");
    core.kill();
    let offline = offline::query(&project);
    assert_eq!(
        offline["instances"]["store"]["installed_reference"],
        installed["value"]
    );
    let mut restarted = Process::start(&project, "retirement-restart");
    version(&project, "0.2.0");
    active(&project, "client");
    assert_eq!(
        send(&project, 5, "fixture.counter@1.0", "get", json!([]))["value"],
        63
    );
    restarted.kill();
    case.success();
}

/// ADR-0031：坏候选恢复整个窗口，联合新接口与新配置在同一事务中生效。
#[test]
fn batch_restores_configuration_and_accepts_joint_interface_change() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "batch");
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "batch", "v1");
    plugin(&project.join("sources/client-fail"), "client", "fail");
    plugin(&project.join("sources/store-next"), "store", "next");
    plugin(&project.join("sources/client-next"), "client", "next");
    let mut core = Process::start(&project, "batch");
    active(&project, "admin");
    active(&project, "client");
    assert_eq!(
        send(&project, 1, "fixture.counter@1.0", "set", json!([42]))["ok"],
        true
    );
    let store = manage(
        &project,
        2,
        "install",
        json!([project.join("sources/store-v2")]),
    )["value"]
        .clone();
    let failed = manage(
        &project,
        3,
        "install",
        json!([project.join("sources/client-fail")]),
    )["value"]
        .clone();
    let configuration = fs::read_to_string(project.join("cordis.toml")).unwrap();
    fs::write(
        project.join("cordis.toml"),
        configuration.replace("initial = 17", "initial = 999"),
    )
    .unwrap();
    let rejected = manage(
        &project,
        4,
        "replace_batch",
        json!([
            [["store", store], ["client", failed]],
            true,
            "joint-rejected"
        ]),
    );
    assert_eq!(rejected["code"], 1001, "{rejected}");
    version(&project, "0.1.0");
    assert_eq!(
        send(&project, 5, "fixture.counter@1.0", "get", json!([]))["value"],
        42
    );
    assert_eq!(
        send(&project, 6, "fixture.counter@1.0", "initial", json!([]))["value"],
        17
    );
    let store = manage(
        &project,
        7,
        "install",
        json!([project.join("sources/store-next")]),
    )["value"]
        .clone();
    let client = manage(
        &project,
        8,
        "install",
        json!([project.join("sources/client-next")]),
    )["value"]
        .clone();
    let accepted = manage(
        &project,
        9,
        "replace_batch",
        json!([
            [["store", store], ["client", client]],
            true,
            "joint-accepted"
        ]),
    );
    assert_eq!(accepted["ok"], true, "{accepted}");
    version(&project, "0.2.0");
    assert_eq!(
        send(&project, 10, "fixture.counter@2.0", "get", json!([]))["value"],
        42
    );
    assert_eq!(
        send(&project, 11, "fixture.counter@2.0", "initial", json!([]))["value"],
        999
    );
    assert_eq!(manage(&project, 12, "shutdown", json!([]))["ok"], true);
    core.finish();
    case.success();
}

#[test]
fn startup_applies_toml_and_preserves_the_previous_deployment_on_rejection() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "startup-config");
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "startup-config", "v1");
    plugin(&project.join("plugins/rejected"), "store", "fail");
    let path = project.join("cordis.toml");
    let configuration = fs::read_to_string(&path).unwrap();
    let running = || {
        until("启动时的配置决议已经发布", || {
            let snapshot = status(&project)?;
            (snapshot["phase"] == "running").then_some(snapshot)
        })
    };

    fs::write(
        &path,
        configuration.split("[instance.store]").next().unwrap(),
    )
    .unwrap();
    let mut empty = Process::start(&project, "startup-empty");
    assert_eq!(running()["instances"], json!([]));
    empty.kill();

    fs::write(&path, &configuration).unwrap();
    let mut configured = Process::start(&project, "startup-configured");
    let accepted = running();
    assert!(instance(&accepted, "store").is_some(), "{accepted}");
    active(&project, "client");
    assert_eq!(
        send(&project, 1, "fixture.counter@1.0", "set", json!([42]))["ok"],
        true
    );
    checkpoint(&project, 42);
    let source = offline::query(&project)["instances"]["store"].clone();
    configured.kill();

    fs::write(
        &path,
        configuration.replace("plugin = 'store'", "plugin = 'rejected'"),
    )
    .unwrap();
    let mut rejected = Process::start(&project, "startup-rejected");
    let snapshot = running();
    assert!(
        snapshot["configuration_error"]
            .as_str()
            .is_some_and(|error| error.contains("fixture apply failed")),
        "{snapshot}"
    );
    active(&project, "client");
    assert_eq!(
        send(&project, 2, "fixture.counter@1.0", "get", json!([]))["value"],
        42
    );
    assert_eq!(offline::query(&project)["instances"]["store"], source);
    rejected.kill();

    fs::write(
        &path,
        configuration.replace("plugin = 'store'", "plugin = 42"),
    )
    .unwrap();
    let mut invalid = Process::start(&project, "startup-invalid");
    running();
    let output = Command::new(env!("CARGO_BIN_EXE_cordis"))
        .args(["status", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let view: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(view["configuration_error"].is_string(), "{view}");
    active(&project, "admin");
    fs::write(&path, configuration.replace("initial = 17", "initial = 29")).unwrap();
    assert_eq!(manage(&project, 3, "reload", json!([]))["ok"], true);
    assert_eq!(
        send(&project, 4, "fixture.counter@1.0", "initial", json!([]))["value"],
        29
    );
    assert_eq!(manage(&project, 5, "shutdown", json!([]))["ok"], true);
    invalid.finish();

    fs::write(&path, "[runtime]\nstate_dir = 42\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cordis"))
        .args(["run", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("state_dir"));
    case.success();
}

/// 批次持久决定包含新基础配置和程序引用，旧进程退役故障保留在报告中。
#[test]
fn batch_keeps_new_configuration_after_committed_retirement_failure() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "batch-retirement");
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "batch-retirement", "shutdown-fail");
    let mut core = Process::start(&project, "batch-retirement");
    active(&project, "admin");
    active(&project, "client");
    assert_eq!(
        send(&project, 1, "fixture.counter@1.0", "set", json!([42]))["ok"],
        true
    );
    let installed = manage(
        &project,
        2,
        "install",
        json!([project.join("sources/store-v2")]),
    )["value"]
        .clone();
    let configuration = fs::read_to_string(project.join("cordis.toml")).unwrap();
    fs::write(
        project.join("cordis.toml"),
        configuration.replace("initial = 17", "initial = 999"),
    )
    .unwrap();
    let result = manage(
        &project,
        3,
        "replace_batch",
        json!([[["store", installed]], true, "retirement"]),
    );
    assert_eq!(result["ok"], true, "{result}");
    let report = until("旧进程退役错误进入换代报告", || {
        let status = status(&project)?;
        status["last_replace"]
            .as_array()?
            .iter()
            .find(|item| item["group"] == "fixture-store" && item["failure"]["code"] == 1003)
            .cloned()
    });
    assert_eq!(report["committed"], true);
    version(&project, "0.2.0");
    assert_eq!(
        send(&project, 4, "fixture.counter@1.0", "get", json!([]))["value"],
        42
    );
    assert_eq!(
        send(&project, 5, "fixture.counter@1.0", "initial", json!([]))["value"],
        999
    );
    core.kill();
    case.success();
}

/// 自定义标识复用保留结果，自动票据跨重启保持不同身份。
#[test]
fn batch_tickets_report_results_and_resubmit_after_core_restart() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "batch-tickets");
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "batch-tickets", "v1");
    let mut core = Process::start(&project, "tickets-first");
    active(&project, "admin");
    active(&project, "client");
    let first_auto = send(&project, 102, "cordis.manage@2.0", "reload", json!([]));
    assert_eq!(first_auto["ok"], true, "{first_auto}");
    let installed = manage(
        &project,
        1,
        "install",
        json!([project.join("sources/store-v2")]),
    )["value"]
        .clone();
    let arguments = json!([[["store", installed]], false, "once"]);
    assert_eq!(
        manage(&project, 2, "replace_batch", arguments.clone())["ok"],
        true
    );
    version(&project, "0.2.0");
    let direct = send(
        &project,
        101,
        "cordis.manage@2.0",
        "result",
        json!(["once"]),
    );
    assert_eq!(direct["value"]["state"], "succeeded", "{direct}");
    // 当前核心保留该调用方的结果，重投同一标识取得原票据。
    assert_eq!(
        manage(&project, 3, "replace_batch", arguments.clone())["ok"],
        true
    );
    version(&project, "0.2.0");
    let configuration = fs::read(project.join("cordis.toml")).unwrap();
    fs::write(project.join("cordis.toml"), "[invalid").unwrap();
    let rejected = manage(
        &project,
        4,
        "replace_batch",
        json!([[], true, "invalid-config"]),
    );
    assert_eq!(rejected["code"], 2, "{rejected}");
    fs::write(project.join("cordis.toml"), configuration).unwrap();
    core.kill();
    core = Process::start(&project, "tickets-restarted");
    active(&project, "admin");
    let next_auto = send(&project, 103, "cordis.manage@2.0", "reload", json!([]));
    assert_eq!(next_auto["ok"], true, "{next_auto}");
    assert_ne!(first_auto["value"]["id"], next_auto["value"]["id"]);
    assert_eq!(manage(&project, 5, "replace_batch", arguments)["ok"], true);
    version(&project, "0.2.0");
    assert_eq!(manage(&project, 6, "shutdown", json!([]))["ok"], true);
    core.finish();
    case.success();
}

#[test]
fn shutdown_ticket_is_accepted_while_state_export_is_waiting() {
    let case = TestGuard::for_package(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "shutdown-during-export",
    );
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "shutdown-during-export", "v1");
    let gate = project.join("export.continue");
    let config = project.join("cordis.toml");
    let settings = fs::read_to_string(&config)
        .unwrap()
        .replace("checkpoint_ms = 100", "checkpoint_ms = 0")
        .replace("lifecycle_timeout_ms = 1000", "lifecycle_timeout_ms = 8000")
        .replace(
            "initial = 17",
            &format!("initial = 17, export_gate = {}", quoted(&gate)),
        );
    fs::write(config, settings).unwrap();
    let mut core = Process::start(&project, "waiting");
    active(&project, "admin");
    active(&project, "client");
    let change = send(
        &project,
        1,
        "cordis.manage@2.0",
        "configure",
        json!(["store", {"initial": 19}]),
    );
    assert_eq!(change["ok"], true, "{change}");
    until("状态导出已冻结并等待继续", || {
        gate.with_extension("started").is_file().then_some(())
    });
    let stop = manage(&project, 2, "shutdown", json!([]));
    assert_eq!(stop["ok"], true, "{stop}");
    until("状态导出等待期间发布停机意图", || {
        let published = status(&project)?;
        (published["phase"] == "stopping").then_some(())
    });
    fs::write(gate, []).unwrap();
    core.finish();
    let stopped = status(&project).unwrap();
    assert_eq!(stopped["phase"], "stopped");
    let outcomes = stopped["last_batch"].as_array().unwrap();
    for ticket in [change, stop] {
        assert!(
            outcomes
                .iter()
                .any(|row| row["id"] == ticket["value"]["id"] && row["state"] == "succeeded"),
            "{stopped}"
        );
    }
    case.success();
}

#[test]
fn reload_rediscovers_moved_source_and_keeps_the_running_copy() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "moved-source");
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "moved-source", "v1");
    let mut core = Process::start(&project, "rename");
    active(&project, "admin");
    let first = active(&project, "store");
    let installed = instance(&first, "store").unwrap()["reference"].clone();
    let moved = project.join("plugins/moved-store");
    fs::rename(project.join("plugins/store"), &moved).unwrap();
    plugin(&moved, "store", "v2");
    assert_eq!(manage(&project, 1, "reload", json!([]))["ok"], true);
    let snapshot = until("renamed source visible", || {
        let snapshot = status(&project)?;
        snapshot["available_plugins"]
            .as_array()?
            .iter()
            .any(|item| item["reference"] == "moved-store")
            .then_some(snapshot)
    });
    assert!(
        !snapshot["available_plugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["reference"] == "store")
    );
    assert_eq!(
        instance(&snapshot, "store").unwrap()["reference"],
        installed
    );
    assert_eq!(group_version(&snapshot, "fixture-store"), "0.1.0");
    let path = project.join("cordis.toml");
    let mut declared: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
    declared["instance"]["store"]["plugin"] = "fixture-store".into();
    fs::write(path, toml::to_string(&declared).unwrap()).unwrap();
    assert_eq!(manage(&project, 2, "reload", json!([]))["ok"], true);
    version(&project, "0.2.0");
    assert_eq!(manage(&project, 3, "shutdown", json!([]))["ok"], true);
    core.finish();
    case.success();
}

/// DOC-11 §10：相同基础来源的实例各自恢复确认版本，来源记录保持配置重读能力。
#[test]
fn confirmed_sources_preserve_group_versions_without_shadowing_configuration() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "source-bindings");
    let _fixture = fixture().for_case(&case);
    let project = source_binding_project(&case);
    let mut core = Process::start(&project, "sources-first");
    active(&project, "admin");
    active(&project, "other");
    let installed = manage(
        &project,
        1,
        "install",
        json!([project.join("sources/store-v2")]),
    )["value"]
        .clone();
    assert_eq!(
        manage(&project, 2, "replace", json!(["store", installed]))["ok"],
        true
    );
    until("first group reaches its independent version", || {
        let snapshot = status(&project)?;
        (group_version(&snapshot, "group-a") == "0.2.0").then_some(())
    });
    plugin(&project.join("plugins/store"), "store", "fail");
    core.kill();
    let mut core = Process::start(&project, "sources-restarted");
    active(&project, "admin");
    active(&project, "store");
    let snapshot = active(&project, "other");
    assert_eq!(group_version(&snapshot, "group-a"), "0.2.0");
    assert_eq!(group_version(&snapshot, "group-b"), "0.1.0");
    let mut configuration: toml::Value =
        toml::from_str(&fs::read_to_string(project.join("cordis.toml")).unwrap()).unwrap();
    configuration["instance"]["other"]["config"]["initial"] = toml::Value::Integer(73);
    fs::write(
        project.join("cordis.toml"),
        toml::to_string_pretty(&configuration).unwrap(),
    )
    .unwrap();
    assert_eq!(manage(&project, 3, "reload", json!([]))["ok"], true);
    assert_eq!(
        send(
            &project.join("b"),
            1,
            "fixture.counter@1.0",
            "initial",
            json!([])
        )["value"],
        73
    );
    assert_eq!(manage(&project, 4, "shutdown", json!([]))["ok"], true);
    core.finish();
    offline::check_stopped_bindings(&project, &snapshot);
    case.success();
}

/// ADR-0025、DOC-11 §10：两个进程组和隔离域使用同一来源，提供可独立替换的服务。
fn source_binding_project(case: &TestGuard) -> PathBuf {
    let project = managed_project(case, "source-bindings", "v1");
    let mut configuration: toml::Value =
        toml::from_str(&fs::read_to_string(project.join("cordis.toml")).unwrap()).unwrap();
    let instances = configuration["instance"].as_table_mut().unwrap();
    let store = instances.get_mut("store").unwrap().as_table_mut().unwrap();
    store.insert("group".into(), "group-a".into());
    store.insert(
        "isolate".into(),
        toml::from_str::<toml::Value>("'fixture.counter@1' = 'tenant-a'").unwrap(),
    );
    let mut other = store.clone();
    other.insert("group".into(), "group-b".into());
    other["config"]["initial"] = toml::Value::Integer(29);
    other.insert(
        "isolate".into(),
        toml::from_str::<toml::Value>("'fixture.counter@1' = 'tenant-b'").unwrap(),
    );
    instances.insert("other".into(), toml::Value::Table(other));
    let client = instances.get_mut("client").unwrap().as_table_mut().unwrap();
    client.insert(
        "isolate".into(),
        toml::from_str::<toml::Value>("'fixture.counter@1' = 'tenant-a'").unwrap(),
    );
    let mut other_client = client.clone();
    other_client.insert(
        "isolate".into(),
        toml::from_str::<toml::Value>("'fixture.counter@1' = 'tenant-b'").unwrap(),
    );
    other_client["config"]["commands"] = project
        .join("b/client-commands")
        .to_string_lossy()
        .into_owned()
        .into();
    other_client["config"]["results"] = project
        .join("b/client-results")
        .to_string_lossy()
        .into_owned()
        .into();
    instances.insert("client-b".into(), toml::Value::Table(other_client));
    fs::write(
        project.join("cordis.toml"),
        toml::to_string_pretty(&configuration).unwrap(),
    )
    .unwrap();
    project
}

/// DOC-15 §4：版本观测来自实际分组握手描述符。
fn group_version<'a>(snapshot: &'a Value, group: &str) -> &'a str {
    snapshot["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == group)
        .unwrap()["plugins"][0]["version"]
        .as_str()
        .unwrap()
}

/// M12：为管理恢复场景建立真实进程插件与有授权的调用方。
#[test]
fn unrelated_batch_preserves_last_checkpoint_on_export_error() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "partial-export");
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "partial-export", "v1");
    let source = project.join("saved-counter");
    fs::write(&source, "137").unwrap();
    let config_path = project.join("cordis.toml");
    let mut config: toml::Table = fs::read_to_string(&config_path).unwrap().parse().unwrap();
    config["instance"]["store"]["config"]
        .as_table_mut()
        .unwrap()
        .insert(
            "checkpoint_source".into(),
            toml::Value::String(source.display().to_string()),
        );
    fs::write(config_path, toml::to_string(&config).unwrap()).unwrap();
    let mut core = Process::start(&project, "partial-export");
    active(&project, "admin");
    checkpoint(&project, 137);
    fs::remove_file(&source).unwrap();
    let change = manage(
        &project,
        1,
        "configure",
        json!(["client", {
            "commands": project.join("client-commands"), "results": project.join("client-results"), "initial": 8
        }]),
    );
    assert_eq!(change["ok"], true, "{change}");
    let saved =
        fs::read(project.join("state/store.state")).expect("其他实例导出失败时保留已发布状态");
    assert_eq!(
        storage::parse_state(&saved).unwrap().data,
        cordis_up::Value::from(137u64)
    );
    fs::write(source, "137").unwrap();
    assert_eq!(manage(&project, 2, "shutdown", json!([]))["ok"], true);
    core.finish();
    case.success();
}

fn managed_project(case: &TestGuard, name: &str, mode: &str) -> PathBuf {
    let project = project(case, name);
    plugin(&project.join("plugins/store"), "store", mode);
    plugin(&project.join("plugins/admin"), "admin", "v1");
    plugin(&project.join("plugins/client"), "client", "v1");
    plugin(&project.join("sources/store-v2"), "store", "v2");
    let mut configuration = format!(
        "[runtime]\ncheckpoint_ms = 100\nconfirm_ms = 0\nlifecycle_timeout_ms = 1000\n\n[instance.store]\nplugin = 'store'\nconfig = {{ initial = 17 }}\n\n[instance.admin]\nplugin = 'admin'\ngrants = {{ 'cordis.manage@1' = ['*'] }}\nconfig = {{ commands = {}, results = {} }}\n",
        quoted(&project.join("commands")),
        quoted(&project.join("results")),
    );
    configuration.push_str(&client_configuration(&project));
    fs::write(project.join("cordis.toml"), configuration).unwrap();
    project
}

/// DOC-11 SW-6：计数器客户端按声明依赖重建，管理调用方保持独立。
fn client_configuration(project: &Path) -> String {
    format!(
        "\n[instance.client]\nplugin = 'client'\nconfig = {{ commands = {}, results = {} }}\n",
        quoted(&project.join("client-commands")),
        quoted(&project.join("client-results")),
    )
}

// M12、ADR-0025：部署隔离域连接相应提供方，拦截元数据进入提供方的scoped视图。
#[test]
fn deployment_realms_resolve_separate_providers() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "realms");
    let _fixture = fixture().for_case(&case);
    let project = project(&case, "realms");
    plugin(&project.join("plugins/store"), "store", "v1");
    plugin(&project.join("plugins/reader"), "reader", "v1");
    let configuration = format!(
        "[runtime]\ncheckpoint_ms = 0\nconfirm_ms = 0\n\n[instance.a]\nplugin = 'store'\nisolate = {{ 'fixture.counter@1' = 'tenant-a' }}\nconfig = {{ initial = 11, scoped = true }}\n\n[instance.b]\nplugin = 'store'\nisolate = {{ 'fixture.counter@1' = 'tenant-b' }}\nconfig = {{ initial = 29, scoped = true }}\n\n[instance.read-a]\nplugin = 'reader'\nisolate = {{ 'fixture.counter@1' = 'tenant-a' }}\nintercept = {{ 'fixture.counter@1' = {{ namespace = 'alpha' }} }}\nconfig = {{ observed = {}, meta_observed = {} }}\n\n[instance.read-b]\nplugin = 'reader'\nisolate = {{ 'fixture.counter@1' = 'tenant-b' }}\nconfig = {{ observed = {}, meta_observed = {} }}\n",
        quoted(&project.join("a.json")),
        quoted(&project.join("meta-a.json")),
        quoted(&project.join("b.json")),
        quoted(&project.join("meta-b.json"))
    );
    fs::write(project.join("cordis.toml"), configuration).unwrap();
    let mut core = Process::start(&project, "realms");
    let a = until("tenant-a value", || {
        fs::read_to_string(project.join("a.json")).ok()
    });
    let b = until("tenant-b value", || {
        fs::read_to_string(project.join("b.json")).ok()
    });
    assert_eq!(serde_json::from_str::<u64>(&a).unwrap(), 11);
    assert_eq!(serde_json::from_str::<u64>(&b).unwrap(), 29);
    let meta_a = until("intercept reaches provider", || {
        fs::read_to_string(project.join("meta-a.json")).ok()
    });
    let meta_b = until("default metadata reaches provider", || {
        fs::read_to_string(project.join("meta-b.json")).ok()
    });
    assert_eq!(
        serde_json::from_str::<Value>(&meta_a).unwrap(),
        json!({ "namespace": "alpha", "cordis.caller": { "instance": "read-a" } })
    );
    assert_eq!(
        serde_json::from_str::<Value>(&meta_b).unwrap(),
        json!({"cordis.caller":{"instance":"read-b"}})
    );
    let pid = core.0.as_ref().unwrap().id();
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    core.finish();
    case.success();
}
