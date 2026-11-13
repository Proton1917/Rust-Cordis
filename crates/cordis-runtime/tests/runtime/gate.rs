//! M13、DOC-11 SW-2：真实验证印记进入运行时安装、替换及宿主重启。

use super::*;
use std::collections::BTreeSet;

struct Artifacts {
    tools: PathBuf,
    broken: PathBuf,
}

/// Y-3：测试自行构建核心、宿主、开发工具和真实DLL，产物目录固定在当前工作区。
fn artifacts(case: &TestGuard) -> Artifacts {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let _build = cordis_testkit::build::shared_build_lock(&root).unwrap();
    let tools = cordis_testkit::build::shared_target_dir(&root);
    checked(
        Command::new(env!("CARGO"))
            .current_dir(&root)
            .args([
                "build",
                "-p",
                "cordis-runtime",
                "-p",
                "cordis-host",
                "-p",
                "cordis-dev",
                "-p",
                "kv-mem",
                "-p",
                "kv-file",
                "-p",
                "kv-contract",
                "--target-dir",
            ])
            .arg(&tools),
    );
    checked(
        Command::new(env!("CARGO"))
            .args(["build", "--manifest-path"])
            .arg(root.join("crates/cordis-dev/tests/fixtures/kv-broken/Cargo.toml"))
            .arg("--target-dir")
            .arg(&tools),
    );
    let names = ["kv_mem", "kv_file", "kv_contract", "kv_broken"].map(|name| {
        format!(
            "{}{name}{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        )
    });
    let copied = cordis_testkit::build::snapshot_artifacts(
        case,
        "runtime-gate",
        "debug",
        &[
            "cordis",
            "cordis-host",
            "cordis-dev",
            &names[0],
            &names[1],
            &names[2],
            &names[3],
        ],
    )
    .unwrap();
    Artifacts {
        broken: library(&copied, "kv_broken"),
        tools: copied,
    }
}

pub(super) fn checked(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(super) fn library(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!(
        "{}{}{}",
        std::env::consts::DLL_PREFIX,
        name,
        std::env::consts::DLL_SUFFIX
    ))
}

/// DOC-11 §8：印记由真实核心执行契约生成，验证副本属于本部署项目。
fn verify(artifacts: &Artifacts, project: &Path, source: &Path) {
    checked(
        Command::new(artifacts.tools.join("cordis-dev"))
            .args(["verify", "--project"])
            .arg(project)
            .arg("--plugin")
            .arg(source)
            .arg("--suite")
            .arg(library(&artifacts.tools, "kv_contract"))
            .arg("--config")
            .arg(project.join("file.toml")),
    );
}

fn installed_libraries(project: &Path) -> BTreeSet<String> {
    fs::read_dir(project.join("plugins"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| {
            name.starts_with("installed-") && name.ends_with(std::env::consts::DLL_SUFFIX)
        })
        .collect()
}

/// SW-2、ADR-0017：普通进程插件调用管理与KV接口，原生库始终受验证门约束。
fn deployment(case: &TestGuard, artifacts: &Artifacts) -> (PathBuf, PathBuf) {
    let project = project(case, "verified-management");
    plugin(&project.join("plugins/admin"), "admin", "v1");
    plugin(&project.join("plugins/client"), "kv-client", "v1");
    let memory = library(&project.join("plugins"), "memory");
    let file = library(&project.join("sources"), "file");
    fs::copy(library(&artifacts.tools, "kv_mem"), &memory).unwrap();
    fs::copy(library(&artifacts.tools, "kv_file"), &file).unwrap();
    fs::write(project.join("file.toml"), "path = 'runtime.cbor'\n").unwrap();
    verify(artifacts, &project, &memory);
    let configuration = format!(
        "[runtime]\nrequire_verified = true\nconfirm_ms = 0\ncheckpoint_ms = 0\nretired_image_limit = 0\n\n[instance.store]\nplugin = {}\ncomponent = 'kv-store'\ngroup = 'kv'\nconfig = {{ path = 'runtime.cbor' }}\n\n[instance.admin]\nplugin = 'admin'\ngrants = {{ 'cordis.manage@1' = ['*'] }}\nconfig = {{ commands = {}, results = {} }}\n\n[instance.client]\nplugin = 'client'\nconfig = {{ commands = {}, results = {} }}\n",
        toml::Value::String(memory.file_name().unwrap().to_string_lossy().into_owned()),
        quoted(&project.join("commands")),
        quoted(&project.join("results")),
        quoted(&project.join("client-commands")),
        quoted(&project.join("client-results")),
    );
    fs::write(project.join("cordis.toml"), configuration).unwrap();
    (project, file)
}

/// SW-2：同字节已安装引用后来取得真实印记，安装同步印记后即可进入准入目录。
fn admit_existing_reference(artifacts: &Artifacts, project: &Path, source: &Path) -> String {
    let before = installed_libraries(project);
    let refused = manage(project, 2, "install", json!([source]));
    assert_eq!(refused["ok"], false, "{refused}");
    let after = installed_libraries(project);
    let installed = after.difference(&before).next().unwrap().clone();
    verify(artifacts, project, source);
    let accepted = manage(project, 3, "install", json!([source]));
    assert_eq!(accepted["ok"], true, "{accepted}");
    assert_eq!(accepted["value"], installed);
    installed
}

/// SW-2、SW-4：真实新字节即使携带旧印记也在打开前拒绝，运行实例的纪元与数据保持。
fn reject_changed_candidate(artifacts: &Artifacts, project: &Path, reference: &str) {
    fs::copy(&artifacts.broken, project.join("plugins").join(reference)).unwrap();
    let before = active(project, "store");
    let revision = instance(&before, "store").unwrap()["revision"].clone();
    let rejected = manage(project, 4, "replace", json!(["store", reference]));
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert!(
        rejected["value"]
            .as_str()
            .unwrap()
            .contains("配置验证目录中缺少与候选完整字节相同的副本"),
        "{rejected}"
    );
    assert_eq!(rejected["detail"]["committed"], false, "{rejected}");
    assert_eq!(
        instance(&active(project, "store"), "store").unwrap()["revision"],
        revision
    );
    assert_eq!(
        send(project, 5, "kv@1.0", "get", json!(["runtime"]))["value"],
        json!([9, 7, 5])
    );
}

#[test]
fn verified_runtime_install_replace_and_threshold_restart_use_real_receipts() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "verified-management");
    let _fixture = fixture().for_case(&case);
    let artifacts = artifacts(&case);
    let (project, file) = deployment(&case, &artifacts);
    let mut core = Process::start_binary(&artifacts.tools.join("cordis"), &project, "gate");
    active(&project, "store");
    active(&project, "admin");
    active(&project, "client");
    let written = send(&project, 1, "kv@1.0", "put", json!(["runtime", [9, 7, 5]]));
    assert_eq!(written["ok"], true, "{written}");
    let reference = admit_existing_reference(&artifacts, &project, &file);
    reject_changed_candidate(&artifacts, &project, &reference);
    let accepted = manage(&project, 6, "install", json!([file]));
    assert_eq!(accepted["ok"], true, "{accepted}");
    let replaced = manage(&project, 7, "replace", json!(["store", accepted["value"]]));
    assert_eq!(replaced["ok"], true, "{replaced}");
    assert_eq!(
        send(&project, 8, "kv@1.0", "get", json!(["runtime"]))["value"],
        json!([9, 7, 5])
    );
    assert_eq!(send(&project, 9, "kv@1.0", "flush", json!([]))["ok"], true);
    let persisted =
        super::isolation::persisted_entries(&project.join("state/data/kv/runtime.cbor"));
    assert_eq!(persisted["runtime"], cordis_up::Value::Bytes(vec![9, 7, 5]));
    assert_eq!(manage(&project, 10, "shutdown", json!([]))["ok"], true);
    core.finish();
    case.success();
}
