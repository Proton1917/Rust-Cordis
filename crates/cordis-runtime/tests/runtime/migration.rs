//! DOC-14 AC-2–AC-5、AC-8：实际 counter 连续写入下的 KV 演化、拒绝与状态恢复。

use super::gate::{checked, library};
use super::*;

struct Artifacts {
    tools: PathBuf,
    memory_v2: PathBuf,
    failed: PathBuf,
    incompatible: PathBuf,
    counter_185: PathBuf,
    consumer: PathBuf,
}

fn artifacts(case: &TestGuard) -> Artifacts {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let target = case.directory("m14-kv");
    let _build = cordis_testkit::build::shared_build_lock(&root).unwrap();
    let shared = cordis_testkit::build::shared_target_dir(&root);
    checked(
        Command::new(env!("CARGO"))
            .current_dir(&root)
            .args([
                "build",
                "-p",
                "cordis-host",
                "-p",
                "counter",
                "-p",
                "kv-mem",
                "-p",
                "kv-file",
                "--target-dir",
            ])
            .arg(&shared),
    );
    let names = ["counter", "kv_mem", "kv_file"].map(|name| {
        format!(
            "{}{name}{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        )
    });
    let tools = cordis_testkit::build::snapshot_artifacts(
        case,
        "migration-tools",
        "debug",
        &["cordis-host", &names[0], &names[1], &names[2]],
    )
    .unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/kv-evolution/Cargo.toml");
    let variants = target.join("variants");
    fs::create_dir_all(&variants).unwrap();
    for (feature, name) in [
        (None, "memory-v2"),
        (Some("fails"), "failed"),
        (Some("incompatible"), "incompatible"),
    ] {
        let mut command = Command::new(env!("CARGO"));
        command
            .args(["build", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&shared);
        if let Some(feature) = feature {
            command.args(["--features", feature]);
        }
        checked(&mut command);
        fs::copy(
            library(&shared.join("debug"), "kv_evolution"),
            library(&variants, name),
        )
        .unwrap();
    }
    checked(
        Command::new("cargo")
            .current_dir(&root)
            .args(["+1.85.0", "build", "-p", "counter", "--target-dir"])
            .arg(&shared),
    );
    checked(
        Command::new("cargo")
            .args(["+1.85.0", "build", "--manifest-path"])
            .arg(root.join("crates/cordis-host/tests/fixtures/Cargo.toml"))
            .args(["-p", "loader-fixture-consumer", "--target-dir"])
            .arg(&shared),
    );
    let consumer = format!(
        "{}loader_fixture_consumer{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let old = cordis_testkit::build::snapshot_artifacts(
        case,
        "migration-185",
        "debug",
        &[&names[0], &consumer],
    )
    .unwrap();
    Artifacts {
        tools,
        memory_v2: library(&variants, "memory-v2"),
        failed: library(&variants, "failed"),
        incompatible: library(&variants, "incompatible"),
        counter_185: library(&old, "counter"),
        consumer: library(&old, "loader_fixture_consumer"),
    }
}

fn deploy(case: &TestGuard, artifacts: &Artifacts) -> PathBuf {
    let project = project(case, "kv-migration");
    fs::copy(env!("CARGO_BIN_EXE_cordis"), project.join("cordis")).unwrap();
    fs::copy(
        artifacts.tools.join("cordis-host"),
        project.join("cordis-host"),
    )
    .unwrap();
    for (name, source) in [
        ("memory", library(&artifacts.tools, "kv_mem")),
        ("counter", library(&artifacts.tools, "counter")),
        ("idle", artifacts.consumer.clone()),
    ] {
        fs::copy(source, library(&project.join("plugins"), name)).unwrap();
    }
    plugin(&project.join("plugins/admin"), "admin", "v1");
    plugin(&project.join("plugins/client"), "kv-client", "v1");
    let config = format!(
        "[runtime]\nconfirm_ms=30\ncheckpoint_ms=100\n\n[instance.store]\nplugin={}\ncomponent='kv-store'\ngroup='application'\nconfig={{path='counter.cbor'}}\n\n[instance.counter]\nplugin='counter'\ncomponent='counter'\nconfig={{interval_ms=5}}\n\n[instance.idle]\nplugin='loader-consumer'\ncomponent='idle'\ngroup='application'\n\n[instance.admin]\nplugin='admin'\ngrants={{'cordis.manage@1'=['*']}}\nconfig={{commands={},results={}}}\n\n[instance.client]\nplugin='client'\nconfig={{commands={},results={}}}\n",
        quoted(library(Path::new(""), "memory").as_path()),
        quoted(&project.join("commands")),
        quoted(&project.join("results")),
        quoted(&project.join("client-commands")),
        quoted(&project.join("client-results"))
    );
    fs::write(project.join("cordis.toml"), config).unwrap();
    project
}

fn next(id: &mut u64) -> u64 {
    *id += 1;
    *id
}

fn count(project: &Path, id: &mut u64) -> u64 {
    until("counter publishes its current KV bytes", || {
        let reply = send(project, next(id), "kv@1.0", "get", json!(["count"]));
        let bytes: Vec<u8> = reply["value"]
            .as_array()?
            .iter()
            .map(|value| value.as_u64().unwrap() as u8)
            .collect();
        Some(u64::from_le_bytes(
            bytes.try_into().expect("counter stores eight bytes"),
        ))
    })
}

fn replace(project: &Path, id: &mut u64, instance: &str, source: &Path) -> Value {
    let local = project.join("sources").join(source.file_name().unwrap());
    fs::copy(source, &local).unwrap();
    let installed = manage(project, next(id), "install", json!([local]));
    assert_eq!(installed["ok"], true, "{installed}");
    manage(
        project,
        next(id),
        "replace",
        json!([instance, installed["value"]]),
    )
}

fn active_build(snapshot: &Value, name: &str, compiler: &str) -> bool {
    snapshot["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|library| {
            library["name"] == name
                && library["retired"] == false
                && library["build_info"]
                    .as_str()
                    .is_some_and(|build| build.contains(compiler))
        })
}

fn evolve_store(project: &Path, id: &mut u64, source: &Path, idle: &Value) -> u64 {
    let before = count(project, id);
    let prior = active(project, "counter");
    let consumer = instance(&prior, "counter").unwrap()["context"].clone();
    let previous_report = prior["last_swap"]
        .as_array()
        .unwrap()
        .iter()
        .find(|report| report["plugin"] == "kv")
        .map(|report| report["candidate_library"].clone());
    let replaced = replace(project, id, "store", source);
    assert_eq!(replaced["ok"], true, "{replaced}");
    let snapshot = until("本次替换的SW-9报告发布", || {
        status(project).filter(|snapshot| {
            instance(snapshot, "counter").is_some_and(|instance| instance["state"] == "active")
                && snapshot["last_swap"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|report| {
                        report["plugin"] == "kv"
                            && report["succeeded"] == true
                            && Some(report["candidate_library"].clone()) != previous_report
                    })
        })
    });
    assert_ne!(instance(&snapshot, "counter").unwrap()["context"], consumer);
    assert_eq!(instance(&snapshot, "idle").unwrap()["context"], *idle);
    let after = count(project, id);
    assert!(after >= before, "计数在状态迁移后保持：{before}→{after}");
    assert!(
        snapshot["last_swap"]
            .as_array()
            .unwrap()
            .iter()
            .any(|report| report["plugin"] == "kv" && report["succeeded"] == true)
    );
    assert!(
        snapshot["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|library| library["name"] == "kv" && library["retired"] == true)
            .all(|library| library["leases"] == 0)
    );
    after
}

fn rejected_versions(project: &Path, id: &mut u64, artifacts: &Artifacts) {
    let before = count(project, id);
    let failed = replace(project, id, "store", &artifacts.failed);
    assert_eq!(failed["ok"], false, "{failed}");
    until(
        "失败候选的回滚报告与 counter Active 一起发布",
        || {
            status(project).filter(|snapshot| {
                instance(snapshot, "counter").is_some_and(|instance| instance["state"] == "active")
                    && snapshot["last_swap"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|report| report["plugin"] == "kv" && report["rolled_back"] == true)
            })
        },
    );
    assert!(count(project, id) >= before);
    let before = active(project, "store");
    let declined = replace(project, id, "store", &artifacts.incompatible);
    assert_eq!(declined["code"], 3, "{declined}");
    let after = active(project, "store");
    for name in ["store", "counter", "idle"] {
        assert_eq!(
            instance(&before, name).unwrap()["context"],
            instance(&after, name).unwrap()["context"],
            "SW-4保留{name}纪元"
        );
    }
}

#[test]
fn continuous_counter_migrates_rolls_back_rejects_and_restores_across_compilers() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "kv-migration");
    let _fixture = fixture().for_case(&case);
    let artifacts = artifacts(&case);
    let project = deploy(&case, &artifacts);
    let mut core = Process::start_binary(&project.join("cordis"), &project, "migration");
    for name in ["store", "counter", "idle", "admin", "client"] {
        active(&project, name);
    }
    let initial = status(&project).unwrap();
    assert!(active_build(&initial, "counter", "1.96.0"));
    let idle = instance(&initial, "idle").unwrap()["context"].clone();
    let mut id = 100;
    let file_value = evolve_store(
        &project,
        &mut id,
        &library(&artifacts.tools, "kv_file"),
        &idle,
    );
    let memory_value = evolve_store(&project, &mut id, &artifacts.memory_v2, &idle);
    rejected_versions(&project, &mut id, &artifacts);
    let before_old_compiler = count(&project, &mut id);
    let changed = replace(&project, &mut id, "counter", &artifacts.counter_185);
    assert_eq!(changed["ok"], true, "{changed}");
    until(
        "Rust 1.85 counter 的活跃库与实例状态发布",
        || {
            status(&project).filter(|snapshot| {
                instance(snapshot, "counter").is_some_and(|instance| instance["state"] == "active")
                    && active_build(snapshot, "counter", "1.85.0")
            })
        },
    );
    let old_compiler_value = count(&project, &mut id);
    assert!(old_compiler_value >= before_old_compiler);
    checked(Command::new("/bin/kill").args(["-TERM", &core.0.as_ref().unwrap().id().to_string()]));
    core.finish();
    let saved =
        storage::parse_state(&fs::read(project.join("state/counter.state")).unwrap()).unwrap();
    let persisted = u64::try_from(saved.data.as_integer().unwrap()).unwrap();
    let mut restarted = Process::start_binary(&project.join("cordis"), &project, "restored");
    active(&project, "counter");
    active(&project, "client");
    let restored = count(&project, &mut id);
    assert!(restored >= persisted && persisted >= old_compiler_value);
    assert!(project.join("state/consumed/counter.state").is_file());
    assert_eq!(
        manage(&project, next(&mut id), "shutdown", json!([]))["ok"],
        true
    );
    restarted.finish();
    fs::write(project.join("acceptance.json"), serde_json::to_vec_pretty(&json!({"file_count":file_value,"memory_v2_count":memory_value,"rust_185_count":old_compiler_value,"shutdown_state":persisted,"restored_count":restored})).unwrap()).unwrap();
    eprintln!("AC-2–5/AC-8证据：{}", case.evidence_for(&project).display());
    case.success();
}
