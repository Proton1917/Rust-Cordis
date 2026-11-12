//! ADR-0033、UPSTREAM-EXT：逻辑组共享隔离域、继承拦截并按禁用状态生成运行目标。

use cordis_runtime::config::{Overlay, OverlayEntry, parse};

const GROUPED: &str = r#"
[scope.tenant]
isolate = { "kv@1" = true }
intercept = { "kv@1" = { namespace = "root", limit = 7 } }

[scope.sub]
parent = "tenant"
intercept = { "kv@1" = { namespace = "child" } }

[scope.other]
isolate = { "kv@1" = true }

[instance.store]
plugin = "kv"
scope = "tenant"
group = "storage-process"

[instance.reader]
plugin = "counter"
scope = "sub"

[instance.other-store]
plugin = "kv"
scope = "other"

[instance.independent]
plugin = "counter"

[instance.off]
plugin = "optional-package"
disabled = true
"#;

#[test]
fn group_views_preserve_shared_realm_and_merge_metadata() {
    let configuration = parse(GROUPED.as_bytes()).unwrap();
    let target = configuration.effective(&Overlay::default()).unwrap();
    assert_eq!(
        target["store"].isolate["kv@1"],
        target["reader"].isolate["kv@1"]
    );
    assert_ne!(
        target["store"].isolate["kv@1"],
        target["other-store"].isolate["kv@1"]
    );
    assert_eq!(target["store"].group.as_deref(), Some("storage-process"));
    let metadata = target["reader"].intercept["kv@1"].as_table().unwrap();
    assert_eq!(metadata["namespace"].as_str(), Some("child"));
    assert_eq!(metadata["limit"].as_integer(), Some(7));
    assert!(!target.contains_key("off"));
}

#[test]
fn ancestor_disable_withdraws_members_and_enable_restores_them() {
    let mut configuration = parse(GROUPED.as_bytes()).unwrap();
    let before = configuration.effective(&Overlay::default()).unwrap();
    configuration.scope.get_mut("tenant").unwrap().disabled = true;
    let disabled = configuration.effective(&Overlay::default()).unwrap();
    assert_eq!(
        disabled.keys().map(String::as_str).collect::<Vec<_>>(),
        ["independent", "other-store"]
    );
    assert_eq!(disabled["independent"], before["independent"]);
    configuration.scope.get_mut("tenant").unwrap().disabled = false;
    assert_eq!(
        configuration.effective(&Overlay::default()).unwrap(),
        before
    );
}

#[test]
fn invalid_group_relationship_is_rejected_before_deployment() {
    assert!(parse(b"[scope.a]\nparent='b'\n[scope.b]\nparent='a'\n").is_err());
    assert!(parse(b"[instance.a]\nplugin='p'\nscope='missing'\n").is_err());
}

#[test]
fn managed_member_preserves_future_ancestor_metadata_changes() {
    let mut configuration = parse(GROUPED.as_bytes()).unwrap();
    let mut overlay = Overlay::default();
    let mut member = configuration.entries(&overlay).remove("reader").unwrap();
    member.config.insert("interval_ms".into(), 42.into());
    overlay
        .instance
        .insert("reader".into(), OverlayEntry::from(&member));
    configuration
        .scope
        .get_mut("tenant")
        .unwrap()
        .intercept
        .get_mut("kv@1")
        .unwrap()
        .as_table_mut()
        .unwrap()
        .insert("limit".into(), 9.into());
    let target = configuration.effective(&overlay).unwrap();
    assert_eq!(
        target["reader"].config["interval_ms"].as_integer(),
        Some(42)
    );
    assert_eq!(
        target["reader"].intercept["kv@1"]["limit"].as_integer(),
        Some(9)
    );
    assert!(overlay.instance["reader"].isolate.is_empty());
}
