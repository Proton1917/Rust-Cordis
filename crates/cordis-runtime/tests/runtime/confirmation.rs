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
        "cordis.manage@1.1",
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
            .find(|outcome| outcome["id"] == "discard" && outcome["state"] == "rejected")
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
