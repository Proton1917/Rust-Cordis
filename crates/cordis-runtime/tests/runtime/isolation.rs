//! DOC-15 AU-18：实际文件 KV 的租户域，以及三种提供方位置的 Scoped 元数据。

use super::gate::{checked, library};
use super::*;

fn tools(name: &str, packages: &[&str]) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let _build = cordis_testkit::build::shared_build_lock(&root).unwrap();
    let target = cordis_testkit::build::shared_target_dir(&root);
    let mut command = Command::new(env!("CARGO"));
    command.current_dir(&root).arg("build");
    for package in packages {
        command.args(["-p", package]);
    }
    command.arg("--target-dir").arg(&target);
    checked(&mut command);
    let names: Vec<_> = packages
        .iter()
        .map(|package| {
            if *package == "cordis-host" {
                package.to_string()
            } else {
                format!(
                    "{}{}{}",
                    std::env::consts::DLL_PREFIX,
                    package.replace('-', "_"),
                    std::env::consts::DLL_SUFFIX
                )
            }
        })
        .collect();
    let files: Vec<_> = names.iter().map(String::as_str).collect();
    cordis_testkit::build::snapshot_artifacts(&root, name, "debug", &files).unwrap()
}

fn core_binaries(project: &Path, host: &Path) {
    fs::copy(env!("CARGO_BIN_EXE_cordis"), project.join("cordis")).unwrap();
    fs::copy(host, project.join("cordis-host")).unwrap();
}

fn finish(core: &mut Process) {
    checked(Command::new("/bin/kill").args(["-TERM", &core.0.as_ref().unwrap().id().to_string()]));
    core.finish();
}

fn decode_file(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    cordis_up::decode_value(&bytes, &cordis_up::Limits::DEFAULT)
        .ok()?
        .deserialized()
        .ok()
}

#[test]
fn real_file_kv_instances_keep_tenant_data_separate() {
    let tools = tools("kv-file", &["cordis-host", "kv-file"]);
    let project = project("real-kv-domains");
    core_binaries(&project, &tools.join("cordis-host"));
    fs::copy(
        library(&tools, "kv_file"),
        library(&project.join("plugins"), "store"),
    )
    .unwrap();
    for tenant in ["a", "b"] {
        plugin(
            &project.join(format!("plugins/client-{tenant}")),
            "kv-client",
            "v1",
        );
    }
    let mut config = String::from("[runtime]\ncheckpoint_ms=0\n");
    for tenant in ["a", "b"] {
        config.push_str(&format!(
            "\n[instance.store-{tenant}]\nplugin='kv'\ncomponent='kv-store'\ngroup='database'\nisolate={{'kv@1'='tenant-{tenant}'}}\nconfig={{path='{tenant}.cbor'}}\n\n[instance.client-{tenant}]\nplugin='client-{tenant}'\ngroup='client-{tenant}'\nisolate={{'kv@1'='tenant-{tenant}'}}\nconfig={{commands={},results={}}}\n",
            quoted(&project.join(format!("{tenant}/client-commands"))), quoted(&project.join(format!("{tenant}/client-results")))));
    }
    fs::write(project.join("cordis.toml"), config).unwrap();
    let mut core = Process::start_binary(&project.join("cordis"), &project, "domains");
    for name in ["store-a", "store-b", "client-a", "client-b"] {
        active(&project, name);
    }
    for (tenant, bytes) in [("a", vec![1, 2]), ("b", vec![9, 8])] {
        assert_eq!(
            send(
                &project.join(tenant),
                1,
                "kv@1.0",
                "put",
                json!(["shared", bytes])
            )["ok"],
            true
        );
    }
    assert_eq!(
        send(&project.join("a"), 2, "kv@1.0", "get", json!(["shared"]))["value"],
        json!([1, 2])
    );
    assert_eq!(
        send(&project.join("b"), 2, "kv@1.0", "get", json!(["shared"]))["value"],
        json!([9, 8])
    );
    for tenant in ["a", "b"] {
        assert_eq!(
            send(&project.join(tenant), 3, "kv@1.0", "flush", json!([]))["ok"],
            true
        );
    }
    assert_eq!(
        decode_file(&project.join("state/data/kv/a.cbor")).unwrap()["entries"]["shared"],
        json!([1, 2])
    );
    assert_eq!(
        decode_file(&project.join("state/data/kv/b.cbor")).unwrap()["entries"]["shared"],
        json!([9, 8])
    );
    finish(&mut core);
    eprintln!("AU-18真实文件KV域证据：{}", project.display());
}

fn scoped_artifacts() -> (PathBuf, PathBuf, PathBuf) {
    let host = tools("scoped-host", &["cordis-host"]).join("cordis-host");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = root.parent().and_then(Path::parent).unwrap();
    let _build = cordis_testkit::build::shared_build_lock(workspace).unwrap();
    let target = cordis_testkit::build::shared_target_dir(workspace);
    for fixture in ["kv-scoped", "kv-scoped-process"] {
        checked(
            Command::new(env!("CARGO"))
                .args(["build", "--manifest-path"])
                .arg(root.join("fixtures").join(fixture).join("Cargo.toml"))
                .arg("--target-dir")
                .arg(&target),
        );
    }
    let native = format!(
        "{}kv_scoped{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let copied = cordis_testkit::build::snapshot_artifacts(
        workspace,
        "scoped-fixtures",
        "debug",
        &[&native, "cordis-kv-scoped-process"],
    )
    .unwrap();
    (
        host,
        library(&copied, "kv_scoped"),
        copied.join("cordis-kv-scoped-process"),
    )
}

fn scoped_project(mode: &str, host: &Path, native: &Path, process: &Path) -> (PathBuf, PathBuf) {
    let project = project(&format!("scoped-kv-{mode}"));
    core_binaries(&project, host);
    fs::copy(native, library(&project.join("plugins"), "scoped")).unwrap();
    let metadata = if mode == "process" {
        project.join("process-metadata.cbor")
    } else {
        project.join("state/data/kv-scoped/metadata.cbor")
    };
    let provider = if mode == "process" {
        let directory = project.join("plugins/process");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("cordis-plugin.toml"), format!("name='kv-scoped-process'\nversion='0.1.0'\nsocket='process'\nprotocol=1\ncommand=[{}]\n", quoted(process))).unwrap();
        format!(
            "plugin='process'\ncomponent='store'\ngroup='provider'\nconfig={{meta_observed={}}}\n",
            quoted(&metadata)
        )
    } else {
        format!(
            "plugin='kv-scoped'\ncomponent='store'\ngroup='{}'\n",
            if mode == "same" { "client" } else { "provider" }
        )
    };
    fs::write(project.join("cordis.toml"), format!(
        "[runtime]\ncheckpoint_ms=0\n\n[instance.provider]\n{provider}\n[instance.client]\nplugin='kv-scoped'\ncomponent='client'\ngroup='client'\nintercept={{'kv@1'={{namespace='{mode}',marker=17}}}}\n")).unwrap();
    (project, metadata)
}

#[test]
fn kv_scoped_metadata_reaches_same_cross_and_process_providers() {
    let (host, native, process) = scoped_artifacts();
    for mode in ["same", "cross", "process"] {
        let (project, metadata) = scoped_project(mode, &host, &native, &process);
        let mut core = Process::start_binary(&project.join("cordis"), &project, mode);
        active(&project, "client");
        let received = until("提供方记录真实Scoped元数据", || {
            decode_file(&metadata)
        });
        assert_eq!(received, json!({"namespace":mode,"marker":17}));
        let observed = until("原生使用方完成实际KV读写", || {
            decode_file(&project.join("state/data/kv-scoped/client.cbor"))
        });
        assert_eq!(observed["typed"], mode == "same");
        assert_eq!(observed["value"], json!([4, 2, 9]));
        assert_eq!(observed["keys"], json!(["probe"]));
        finish(&mut core);
        eprintln!("AU-18 kv@1 {mode}证据：{}", project.display());
    }
}
