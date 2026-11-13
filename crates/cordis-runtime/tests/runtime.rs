use cordis_runtime::storage;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

#[path = "runtime/gate.rs"]
mod gate;

#[path = "runtime/migration.rs"]
mod migration;

#[path = "runtime/isolation.rs"]
mod isolation;

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

fn fixture() -> &'static Path {
    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let package = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = package.parent().and_then(Path::parent).unwrap();
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
            workspace,
            "runtime-fixture",
            "debug",
            &["cordis-runtime-fixture"],
        )
        .unwrap()
        .join("cordis-runtime-fixture")
    })
}

fn project(name: &str) -> PathBuf {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = package.parent().and_then(Path::parent).unwrap();
    let project = workspace
        .join("target/runtime-m12/cases")
        .join(format!("{name}-{}", std::process::id()));
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
    let reply = send(project, id, "cordis.manage@1.1", method, args.clone());
    if method != "replace_batch" || reply["ok"] != true {
        return reply;
    }
    let request_id = args[2].as_str().unwrap();
    assert_eq!(reply["value"]["id"], request_id, "{reply}");
    let outcome = until("batch reaches a durable final result", || {
        let snapshot = status(project)?;
        snapshot["last_batch"]
            .as_array()?
            .iter()
            .find(|outcome| {
                outcome["caller"] == "admin"
                    && outcome["id"] == request_id
                    && matches!(outcome["state"].as_str(), Some("succeeded" | "rejected"))
            })
            .cloned()
    });
    let records: Value =
        serde_json::from_slice(&fs::read(project.join("state/batches.json")).unwrap()).unwrap();
    let record = &records["admin"];
    assert_eq!(record["id"], request_id);
    json!({ "ok": outcome["state"] == "succeeded", "code": record["error"]["code"],
        "detail": record["error"]["detail"], "value": outcome["error"] })
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
    let project = project("management");
    plugin(&project.join("plugins/store"), "store", "v1");
    plugin(&project.join("plugins/admin"), "admin", "v1");
    plugin(&project.join("plugins/client"), "client", "v1");
    plugin(&project.join("sources/store-v2"), "store", "v2");
    let mut configuration = format!(
        "requires_core = '^0.1'\n[runtime]\ncheckpoint_ms = 100\nconfirm_ms = 20\napply_timeout_ms = 3000\ncleanup_timeout_ms = 3000\nswap_timeout_ms = 5000\n\n[instance.store]\nplugin = 'store'\nconfig = {{ initial = 41, writes = 5 }}\n\n[instance.admin]\nplugin = 'admin'\ngrants = {{ 'cordis.manage@1' = ['install', 'replace', 'rollback', 'configure', 'reload', 'shutdown'] }}\nconfig = {{ commands = {}, results = {} }}\n\n[instance.denied]\nplugin = 'admin'\nconfig = {{ commands = {}, results = {} }}\n",
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
    plugin(&project.join("plugins/store"), "store", "fail");
    assert_eq!(
        manage(&project, 8, "rollback", json!(["store"]))["ok"],
        true
    );
    version(&project, "0.1.0");
    checkpoint(&project, 46);
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
    let mut core = Process::start(&project, "restarted");
    active(&project, "admin");
    version(&project, "0.2.0");
    checkpoint(&project, 137);
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
    checkpoint(&project, 46);
    let shutdown = manage(&project, 10, "shutdown", json!([]));
    assert_eq!(shutdown["ok"], true, "{shutdown}");
    core.finish();
    let persisted =
        storage::parse_state(&fs::read(project.join("state/store.state")).unwrap()).unwrap();
    assert_eq!(
        u64::try_from(persisted.data.as_integer().unwrap()).unwrap(),
        46
    );
}

/// DOC-11 §7：空原快照在回滚时恢复组件初值，并清除新版本检查点。
#[test]
fn rollback_restores_absent_state_after_new_version_exports() {
    let project = managed_project("empty-rollback", "stateless");
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
    let restored = send(&project, 5, "fixture.counter@1.0", "get", json!([]));
    assert_eq!(restored["value"], 17, "{restored}");
    assert_eq!(manage(&project, 6, "shutdown", json!([]))["ok"], true);
    core.finish();
    let mut core = Process::start(&project, "empty-restarted");
    active(&project, "admin");
    let restored = send(&project, 7, "fixture.counter@1.0", "get", json!([]));
    assert_eq!(restored["value"], 17, "{restored}");
    assert_eq!(manage(&project, 8, "shutdown", json!([]))["ok"], true);
    core.finish();
}

/// DOC-11 SW-9：旧进程拒绝退役时，已提交的新版本跨核心重启继续生效。
#[test]
fn committed_replacement_error_persists_active_version() {
    let project = managed_project("committed-retirement", "shutdown-fail");
    let mut core = Process::start(&project, "retirement-first");
    active(&project, "admin");
    let installed = manage(
        &project,
        1,
        "install",
        json!([project.join("sources/store-v2")]),
    );
    assert_eq!(installed["ok"], true, "{installed}");
    let replaced = manage(&project, 2, "replace", json!(["store", installed["value"]]));
    assert_eq!(replaced["code"], 1003, "{replaced}");
    assert_eq!(replaced["detail"]["committed"], true, "{replaced}");
    version(&project, "0.2.0");
    let overlay = storage::Store::create(project.join("state"))
        .unwrap()
        .overlay()
        .unwrap();
    assert_eq!(
        overlay.instance["store"].plugin,
        installed["value"].as_str().unwrap()
    );
    let written = send(&project, 3, "fixture.counter@1.0", "set", json!([63]));
    assert_eq!(written["ok"], true, "{written}");
    checkpoint(&project, 63);
    assert_eq!(manage(&project, 4, "shutdown", json!([]))["ok"], true);
    core.finish();
    let mut core = Process::start(&project, "retirement-restarted");
    active(&project, "admin");
    version(&project, "0.2.0");
    let restored = send(&project, 5, "fixture.counter@1.0", "get", json!([]));
    assert_eq!(restored["value"], 63, "{restored}");
    assert_eq!(manage(&project, 6, "shutdown", json!([]))["ok"], true);
    core.finish();
}

/// ADR-0031：坏候选恢复整个窗口，联合新接口与新配置在同一事务中生效。
#[test]
fn batch_restores_configuration_and_accepts_joint_interface_change() {
    let project = managed_project("batch", "v1");
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
    assert_eq!(rejected["detail"]["committed"], false, "{rejected}");
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
}

/// ADR-0031、SW-9：确认后的旧进程退役错误也恢复窗口前配置、版本和原快照。
#[test]
fn batch_restores_window_after_committed_retirement_failure() {
    let project = managed_project("batch-retirement", "shutdown-fail");
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
    assert_eq!(result["code"], 1003, "{result}");
    assert_eq!(result["detail"]["committed"], false, "{result}");
    version(&project, "0.1.0");
    assert_eq!(
        send(&project, 4, "fixture.counter@1.0", "get", json!([]))["value"],
        42
    );
    assert_eq!(
        send(&project, 5, "fixture.counter@1.0", "initial", json!([]))["value"],
        17
    );
    core.kill();
}

/// ADR-0031：受理凭据与最终结果分离，同ID复用确定结果，中断后继续同一窗口。
#[test]
fn batch_tickets_replay_results_and_resume_after_core_restart() {
    let project = managed_project("batch-tickets", "v1");
    plugin(&project.join("sources/store-waiting"), "store", "waiting");
    let mut core = Process::start(&project, "tickets-first");
    active(&project, "admin");
    active(&project, "client");
    ticket_replay_and_parse_failure(&project);
    ticket_restart(&project, &mut core);
    assert_eq!(manage(&project, 20, "shutdown", json!([]))["ok"], true);
    core.finish();
}

/// DOC-11 §10：相同基础来源的实例各自恢复确认版本，来源记录保持配置重读能力。
#[test]
fn confirmed_sources_preserve_group_versions_without_shadowing_configuration() {
    let project = source_binding_project();
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
}

/// ADR-0025、DOC-11 §10：两个进程组和隔离域使用同一来源，提供可独立替换的服务。
fn source_binding_project() -> PathBuf {
    let project = managed_project("source-bindings", "v1");
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

/// ADR-0031：已完成ID保持原结果，配置解析错误在返回凭据后成为真实拒绝记录。
fn ticket_replay_and_parse_failure(project: &Path) {
    let installed = manage(
        project,
        1,
        "install",
        json!([project.join("sources/store-v2")]),
    )["value"]
        .clone();
    assert_eq!(
        manage(
            project,
            2,
            "replace_batch",
            json!([[["store", installed]], false, "once"])
        )["ok"],
        true
    );
    let revision = instance(&active(project, "store"), "store").unwrap()["revision"].clone();
    assert_eq!(
        manage(
            project,
            3,
            "replace_batch",
            json!([[["store", "missing"]], false, "once"])
        )["ok"],
        true
    );
    assert_eq!(
        manage(project, 4, "configure", json!(["store", { "initial": 17 }]))["ok"],
        true
    );
    let records: Value =
        serde_json::from_slice(&fs::read(project.join("state/batches.json")).unwrap()).unwrap();
    assert_eq!(records["admin"]["state"], "succeeded");
    assert_eq!(
        instance(&active(project, "store"), "store").unwrap()["revision"],
        revision
    );
    let configuration = fs::read(project.join("cordis.toml")).unwrap();
    fs::write(project.join("cordis.toml"), "[invalid").unwrap();
    let rejected = manage(
        project,
        5,
        "replace_batch",
        json!([[], true, "invalid-config"]),
    );
    assert_eq!(rejected["code"], 2, "{rejected}");
    fs::write(project.join("cordis.toml"), configuration).unwrap();
}

/// CP-7、ADR-0031：真实核心被杀后残留pending变interrupted，依赖就绪后重提原ID。
fn ticket_restart(project: &Path, core: &mut Process) {
    let ready = project.join("ready");
    assert_eq!(
        manage(
            project,
            6,
            "configure",
            json!(["store", { "initial": 17, "ready": ready }])
        )["ok"],
        true
    );
    let installed = manage(
        project,
        7,
        "install",
        json!([project.join("sources/store-waiting")]),
    )["value"]
        .clone();
    let arguments = json!([[["store", installed]], false, "interrupted"]);
    let accepted = send(
        project,
        8,
        "cordis.manage@1.1",
        "replace_batch",
        arguments.clone(),
    );
    assert_eq!(accepted["value"]["id"], "interrupted", "{accepted}");
    let records: Value =
        serde_json::from_slice(&fs::read(project.join("state/batches.json")).unwrap()).unwrap();
    assert_eq!(records["admin"]["state"], "pending");
    core.kill();
    *core = Process::start(project, "tickets-restarted");
    active(project, "admin");
    until("restart publishes interrupted batch", || {
        let snapshot = status(project)?;
        snapshot["last_batch"]
            .as_array()?
            .iter()
            .any(|outcome| outcome["id"] == "interrupted" && outcome["state"] == "interrupted")
            .then_some(())
    });
    fs::write(ready, "ready").unwrap();
    assert_eq!(manage(project, 9, "replace_batch", arguments)["ok"], true);
    assert_eq!(
        send(project, 10, "fixture.counter@1.0", "get", json!([]))["value"],
        17
    );
}

/// M12：为管理恢复场景建立真实进程插件与有授权的调用方。
fn managed_project(name: &str, mode: &str) -> PathBuf {
    let project = project(name);
    plugin(&project.join("plugins/store"), "store", mode);
    plugin(&project.join("plugins/admin"), "admin", "v1");
    plugin(&project.join("plugins/client"), "client", "v1");
    plugin(&project.join("sources/store-v2"), "store", "v2");
    let mut configuration = format!(
        "[runtime]\ncheckpoint_ms = 100\nconfirm_ms = 0\ncleanup_timeout_ms = 200\n\n[instance.store]\nplugin = 'store'\nconfig = {{ initial = 17 }}\n\n[instance.admin]\nplugin = 'admin'\ngrants = {{ 'cordis.manage@1' = ['*'] }}\nconfig = {{ commands = {}, results = {} }}\n",
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
    let project = project("realms");
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
        json!({ "namespace": "alpha" })
    );
    assert_eq!(serde_json::from_str::<Value>(&meta_b).unwrap(), Value::Null);
    let pid = core.0.as_ref().unwrap().id();
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    core.finish();
}
