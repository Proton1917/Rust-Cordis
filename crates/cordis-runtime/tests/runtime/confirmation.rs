//! ADR-0052 R-2：真实批次的确认期丢弃信息进入状态文件与持久批次记录。

use super::*;

#[test]
fn batch_confirmation_discard_is_persisted_and_published() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "batch-discard");
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "batch-discard", "v1");
    let config = fs::read_to_string(project.join("cordis.toml")).unwrap();
    fs::write(
        project.join("cordis.toml"),
        config.replace("confirm_ms = 0", "confirm_ms = 3000"),
    )
    .unwrap();
    let mut core = Process::start(&project, "discard");
    active(&project, "admin");
    active(&project, "client");
    let confirmed = offline::query(&project);
    let reference = manage(
        &project,
        1,
        "install",
        json!([project.join("sources/store-v2")]),
    )["value"]
        .clone();
    let reply = send(
        &project,
        2,
        "cordis.manage@2.0",
        "replace_batch",
        json!([[["store", reference]], false, "discard"]),
    );
    assert_eq!(reply["ok"], true);
    version(&project, "0.2.0");
    assert_eq!(offline::query(&project), confirmed);
    active(&project, "client");
    let written = send(&project, 3, "fixture.counter@1.0", "set", json!([129]));
    assert_eq!(written["ok"], true, "{written}");
    let current = status(&project).unwrap();
    let pid = current["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["name"] == "fixture-store")
        .unwrap()["pid"]
        .as_u64()
        .unwrap();
    assert!(
        Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let outcome = until("confirmation discard published", || {
        status(&project)?["last_batch"]
            .as_array()?
            .iter()
            .find(|outcome| outcome["id"] == "discard" && outcome["state"] == "restored")
            .cloned()
    });
    let discarded = &outcome["discarded_confirmation"];
    assert_eq!(discarded["instances"], json!(["client", "store"]));
    assert_eq!(discarded["configured_ms"], 3000);
    version(&project, "0.1.0");
    assert_eq!(offline::query(&project), confirmed);
    assert_eq!(
        send(&project, 4, "fixture.counter@1.0", "get", json!([]))["value"],
        17
    );
    assert_eq!(manage(&project, 5, "shutdown", json!([]))["ok"], true);
    core.finish();
    case.success();
}

#[test]
fn failed_source_restart_blocks_structure_and_preserves_the_offline_decision() {
    let case = TestGuard::for_package(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "batch-recovery-failed",
    );
    let _fixture = fixture().for_case(&case);
    let project = managed_project(&case, "batch-recovery-failed", "v1");
    let configuration = project.join("cordis.toml");
    let mut config = fs::read_to_string(&configuration)
        .unwrap()
        .replace("confirm_ms = 0", "confirm_ms = 6000");
    config.push_str("\n[instance.client.isolate]\n'fixture.counter@1'='healthy'\n[instance.healthy]\nplugin='healthy'\ngroup='healthy'\nisolate={'fixture.counter@1'='healthy'}\nconfig={initial=91}\n");
    fs::write(configuration, config).unwrap();
    plugin(&project.join("plugins/healthy"), "store", "v1");
    let executable = project.join("previous-store");
    fs::copy(fixture().as_path(), &executable).unwrap();
    let manifest_path = project.join("plugins/store/cordis-plugin.toml");
    let mut manifest: toml::Value =
        toml::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["command"].as_array_mut().unwrap()[0] =
        executable.to_string_lossy().into_owned().into();
    fs::write(manifest_path, toml::to_string(&manifest).unwrap()).unwrap();

    let mut core = Process::start(&project, "failed-recovery");
    active(&project, "admin");
    active(&project, "client");
    let group_pid = |snapshot: &Value, group: &str| {
        snapshot["groups"]
            .as_array()?
            .iter()
            .find(|row| row["name"] == group)?["pid"]
            .as_u64()
    };
    let original = active(&project, "store");
    let previous_pid = group_pid(&original, "fixture-store").unwrap();
    let healthy_pid = group_pid(&original, "healthy").unwrap();
    let installed = manage(
        &project,
        1,
        "install",
        json!([project.join("sources/store-v2")]),
    );
    assert_eq!(installed["ok"], true, "{installed}");
    let committed = offline::query(&project);
    let ticket = send(
        &project,
        2,
        "cordis.manage@2.0",
        "replace_batch",
        json!([[["store", installed["value"]]], false, "recovery-failed"]),
    );
    assert_eq!(ticket["value"]["id"], "recovery-failed", "{ticket}");
    let candidate_pid = until("候选开始运行，确认期保留原进程", || {
        let snapshot = status(&project)?;
        let pid = group_pid(&snapshot, "fixture-store")?;
        (pid != previous_pid && group_version(&snapshot, "fixture-store") == "0.2.0").then_some(pid)
    });
    fs::rename(&executable, executable.with_extension("disabled")).unwrap();
    let signal = |kind: &str, pid: u64| {
        Command::new("/bin/kill")
            .args([kind, &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(signal("-KILL", previous_pid));
    until("原进程已退出", || {
        (!signal("-0", previous_pid)).then_some(())
    });
    assert!(signal("-KILL", candidate_pid));

    until("恢复待处置状态已发布", || {
        let snapshot = status(&project)?;
        (snapshot["phase"] == "recovery-required"
            && snapshot["last_batch"]
                .as_array()?
                .iter()
                .any(|row| row["id"] == "recovery-failed" && row["state"] == "recovery_required"))
        .then_some(())
    });
    let result = send(
        &project,
        3,
        "cordis.manage@2.0",
        "result",
        json!(["recovery-failed"]),
    );
    assert_eq!(result["value"]["state"], "recovery_required", "{result}");
    assert!(
        result["value"]["error"]
            .as_str()
            .unwrap()
            .contains("原状态恢复失败"),
        "{result}"
    );
    assert_eq!(offline::query(&project), committed);
    let blocked = manage(
        &project,
        4,
        "configure",
        json!(["healthy", {"initial": 101}]),
    );
    assert_eq!(blocked["state"], "recovery_required", "{blocked}");
    let healthy = send(&project, 5, "fixture.counter@1.0", "get", json!([]));
    assert_eq!(healthy["value"], 91, "{healthy}");
    assert_eq!(
        group_pid(&status(&project).unwrap(), "healthy"),
        Some(healthy_pid)
    );

    assert!(signal("-TERM", u64::from(core.0.as_ref().unwrap().id())));
    let exit = until("核心报告恢复失败并完成停机", || {
        core.0.as_mut().unwrap().try_wait().unwrap()
    });
    assert_eq!(exit.code(), Some(1));
    core.0.take();
    until("独立业务进程已回收", || {
        (!signal("-0", healthy_pid)).then_some(())
    });
    case.success();
}
