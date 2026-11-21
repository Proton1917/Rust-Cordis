//! ADR-0054：通过真实部署、候选提交和退出验证离线程序绑定契约。

use super::*;
use std::collections::BTreeMap;

#[test]
fn one_core_owns_the_state_directory_and_only_its_snapshot_is_live() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "core-owner");
    let project = case.directory("runtime-owner");
    fs::create_dir_all(&project).unwrap();
    let config = project.join("cordis.toml");
    fs::write(&config, "").unwrap();
    let mut first = Process::start(&project, "owner");
    let mut second = Process(Some(
        Command::new(env!("CARGO_BIN_EXE_cordis"))
            .args(["run", "--config"])
            .arg(&config)
            .spawn()
            .unwrap(),
    ));
    let exit = until("second core refused", || {
        second.0.as_mut().unwrap().try_wait().unwrap()
    });
    assert_eq!(exit.code(), Some(1));
    second.0.take();
    let status = || {
        Command::new(env!("CARGO_BIN_EXE_cordis"))
            .args(["status", "--config"])
            .arg(&config)
            .output()
            .unwrap()
    };
    let live = status();
    assert!(live.status.success());
    let view: Value = serde_json::from_slice(&live.stdout).unwrap();
    assert_eq!(view["pid"], first.0.as_ref().unwrap().id());
    first.kill();
    let stale = status();
    assert_eq!(stale.status.code(), Some(1));
    assert!(stale.stdout.is_empty());
    let mut next = Process::start(&project, "restart");
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &next.0.as_ref().unwrap().id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    next.finish();
    let stopped = status();
    assert!(stopped.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&stopped.stdout).unwrap()["phase"],
        "stopped"
    );
    case.success();
}

#[test]
fn a_fresh_empty_deployment_publishes_empty_confirmed_bindings() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "fresh-offline");
    let project = case.directory("fresh-offline");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("cordis.toml"), "").unwrap();
    let mut core = Process::start(&project, "empty");
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &core.0.as_ref().unwrap().id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    core.finish();
    assert_eq!(query(&project), json!({"schema_version":1,"instances":{}}));
    case.success();
}

fn output(project: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cordis"))
        .args(["status", "--offline", "--config"])
        .arg(project.join("cordis.toml"))
        .current_dir(project.parent().unwrap())
        .output()
        .unwrap()
}

pub(super) fn query(project: &Path) -> Value {
    let output = output(project);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// ADR-0054：期望引用来自运行快照，声明来自真实配置，停机后检查原子记录的只读访问。
pub(super) fn check_stopped_bindings(project: &Path, running: &Value) {
    let configuration = project.join("cordis.toml");
    let mut config: toml::Value =
        toml::from_str(&fs::read_to_string(&configuration).unwrap()).unwrap();
    let instances: BTreeMap<_, _> = running["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|instance| {
            let name = instance["name"].as_str().unwrap();
            (
                name,
                json!({
                    "declared_source": config["instance"][name]["plugin"].as_str().unwrap(),
                    "installed_reference": instance["reference"],
                    "plugin_directory": project.join("plugins").canonicalize().unwrap(),
                }),
            )
        })
        .collect();
    let expected = json!({"schema_version": 1, "instances": instances});
    let before = file_contents(project.join("state"));
    let config_before = file_record(&configuration);
    assert_eq!(query(project), expected);
    assert_eq!(file_contents(project.join("state")), before);
    assert_eq!(file_record(&configuration), config_before);
    let stopped: Value =
        serde_json::from_str(&cordis_runtime::cli::status(&configuration).unwrap()).unwrap();
    assert_eq!(stopped["phase"], "stopped");
    assert_eq!(stopped["instances"], json!([]));
    check_malformed_fields(project);

    // 停机升级改写配置和移动来源后，原绑定继续使用已记录目录。
    let runtime = config["runtime"].as_table_mut().unwrap();
    runtime.insert("plugin_dir".into(), "replacement-plugins".into());
    runtime.insert("state_dir".into(), "persisted".into());
    config["instance"]["store"]["plugin"] = "replacement-store".into();
    fs::write(&configuration, toml::to_string_pretty(&config).unwrap()).unwrap();
    fs::rename(project.join("state"), project.join("persisted")).unwrap();
    fs::rename(project.join("plugins"), project.join("moved-plugins")).unwrap();
    assert_eq!(query(project), expected);
    fs::write(
        project.join("offline-status.json"),
        serde_json::to_vec_pretty(&expected).unwrap(),
    )
    .unwrap();
}

type FileRecord = (Vec<u8>, std::time::SystemTime);

fn file_record(path: &Path) -> FileRecord {
    (
        fs::read(path).unwrap(),
        fs::metadata(path).unwrap().modified().unwrap(),
    )
}

fn file_contents(directory: PathBuf) -> BTreeMap<PathBuf, FileRecord> {
    let mut records = BTreeMap::new();
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            records.extend(file_contents(path));
        } else {
            records.insert(path.clone(), file_record(&path));
        }
    }
    records
}

/// ADR-0054：一次真实批次更新两项绑定，每次并发查询均观察整份旧版或新版。
#[test]
fn offline_queries_observe_atomic_commits_and_empty_deployments() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "offline-commit");
    let _fixture = fixture().for_case(&case);
    let project = source_binding_project(&case);
    set_configuration(&project, |config| {
        config["runtime"]["confirm_ms"] = 400.into();
    });
    let mut core = Process::start(&project, "offline-commit");
    active(&project, "client");
    active(&project, "client-b");
    let previous = query(&project);
    let reference = manage(
        &project,
        1,
        "install",
        json!([project.join("sources/store-v2")]),
    )["value"]
        .clone();
    let mut next = previous.clone();
    for name in ["store", "other"] {
        next["instances"][name]["installed_reference"] = reference.clone();
    }
    observe_batch_commit(&project, reference, &previous, &next);
    check_disabled_and_empty(&project, &next);
    assert!(
        Command::new("/bin/kill")
            .args(["-INT", &core.0.as_ref().unwrap().id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    core.finish();
    check_invalid_records(&project);
    case.success();
}

fn observe_batch_commit(project: &Path, reference: Value, previous: &Value, next: &Value) {
    let reader_project = project.to_owned();
    let old = previous.clone();
    let new = next.clone();
    let (ready, started) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || {
        let initial = query(&reader_project);
        assert_eq!(initial, old);
        ready.send(()).unwrap();
        read_until_commit(&reader_project, initial, &new)
    });
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    let result = manage(
        project,
        2,
        "replace_batch",
        json!([
            [["store", reference], ["other", reference]],
            false,
            "offline-commit"
        ]),
    );
    let observed = reader.join().unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert!(observed.contains(previous), "查询应覆盖确认期的旧版记录");
    assert!(observed.contains(next), "查询应覆盖提交后的新版记录");
    for snapshot in &observed {
        assert!(snapshot == previous || snapshot == next, "{snapshot}");
    }
    assert_eq!(query(project), *next);
    fs::write(
        project.join("offline-commit.json"),
        serde_json::to_vec_pretty(&observed).unwrap(),
    )
    .unwrap();
}

/// ADR-0054：确认开始后在十秒内读取新记录，首个新记录之后的查询保持已提交版本。
fn read_until_commit(project: &Path, initial: Value, next: &Value) -> Vec<Value> {
    let started = Instant::now();
    let mut observed = vec![initial];
    loop {
        let snapshot = query(project);
        let committed = snapshot == *next;
        observed.push(snapshot);
        if committed {
            return observed;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "提交查询截止时间"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn set_configuration(project: &Path, update: impl FnOnce(&mut toml::Value)) {
    let path = project.join("cordis.toml");
    let mut config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    update(&mut config);
    fs::write(path, toml::to_string_pretty(&config).unwrap()).unwrap();
}

/// ADR-0054：停用保留旧绑定，按实际管理操作移除全部实例后持久发布空映射。
fn check_disabled_and_empty(project: &Path, confirmed: &Value) {
    set_configuration(project, |config| {
        config["instance"]["other"]
            .as_table_mut()
            .unwrap()
            .insert("disabled".into(), true.into());
    });
    assert_eq!(manage(project, 3, "reload", json!([]))["ok"], true);
    assert_eq!(query(project), *confirmed);
    for (index, name) in ["client", "client-b", "store", "other", "admin"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            manage(project, 4 + index as u64, "remove", json!([name]))["ok"],
            true
        );
    }
    until(
        "all bindings removed after committed management operations",
        || {
            let observed = query(project);
            (observed == json!({"schema_version": 1, "instances": {}})).then_some(())
        },
    );
}

/// ADR-0054：对真实提交文件施加字段缺失和类型错误，验证公开读取的错误行为。
fn check_malformed_fields(project: &Path) {
    let path = project.join("state/sources.json");
    let original = fs::read(&path).unwrap();
    let records: Value = serde_json::from_slice(&original).unwrap();
    let mut missing = records.clone();
    missing["store"]
        .as_object_mut()
        .unwrap()
        .remove("installed_reference");
    fs::write(&path, serde_json::to_vec(&missing).unwrap()).unwrap();
    assert_query_failure(project);
    let mut wrong_type = records;
    wrong_type["store"]["plugin_directory"] = json!(42);
    fs::write(&path, serde_json::to_vec(&wrong_type).unwrap()).unwrap();
    assert_query_failure(project);
    fs::write(&path, original).unwrap();
}

/// ADR-0054：真实空记录缺席或截断时返回失败，成功空映射仍可在停机后查询。
fn check_invalid_records(project: &Path) {
    let path = project.join("state/sources.json");
    let original = fs::read(&path).unwrap();
    assert_eq!(
        query(project),
        json!({"schema_version": 1, "instances": {}})
    );
    fs::rename(&path, path.with_extension("saved")).unwrap();
    assert_query_failure(project);
    fs::rename(path.with_extension("saved"), &path).unwrap();
    fs::write(&path, b"{").unwrap();
    assert_query_failure(project);
    fs::write(&path, original).unwrap();
}

fn assert_query_failure(project: &Path) {
    let output = output(project);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}
