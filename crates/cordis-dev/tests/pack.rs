//! AU-12：发布包离开构建目录后由附带核心直接运行真实 KV 与计数组件。

use cordis_testkit::cleanup::{FixtureLease, SharedFixture, TestGuard};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Tools {
    root: PathBuf,
    binaries: PathBuf,
}

/// Y-3、AU-12：按当前源码构建运行程序，显式产物目录兼容外置 CARGO_TARGET_DIR。
fn tools() -> FixtureLease<Tools> {
    static TOOLS: SharedFixture<Tools> = SharedFixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    TOOLS.get_or_init(&root, "pack-tools", |guard| {
        let _build = cordis_testkit::build::shared_build_lock(&root).unwrap();
        let target = cordis_testkit::build::shared_target_dir(&root);
        command(
            Command::new(env!("CARGO"))
                .current_dir(&root)
                .args([
                    "build",
                    "--release",
                    "-p",
                    "cordis-runtime",
                    "-p",
                    "cordis-host",
                    "-p",
                    "cordis-dev",
                    "-p",
                    "kv-mem",
                    "--target-dir",
                ])
                .arg(&target),
        );
        let library = format!(
            "{}kv_mem{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        );
        let binaries = cordis_testkit::build::snapshot_artifacts(
            guard,
            "pack-tools",
            "release",
            &["cordis", "cordis-host", "cordis-dev", &library],
        )
        .unwrap();
        Tools {
            root: root.clone(),
            binaries,
        }
    })
}

struct Core(Child);

impl Drop for Core {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}

fn command(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn project(label: &str) -> (PathBuf, PathBuf) {
    let root = tools().root.clone();
    let project = root
        .join("target/pack-tests")
        .join(format!("{label}-{}", std::process::id()));
    fs::create_dir_all(&project).unwrap();
    let sdk = toml::Value::String(
        root.join("crates/cordis-sdk")
            .to_string_lossy()
            .into_owned(),
    );
    let api = toml::Value::String(root.join("plugins/kv-api").to_string_lossy().into_owned());
    let memory = toml::Value::String(
        root.join("plugins/kv-mem-component")
            .to_string_lossy()
            .into_owned(),
    );
    let counter = toml::Value::String(
        root.join("plugins/counter-component")
            .to_string_lossy()
            .into_owned(),
    );
    fs::write(project.join("Cargo.toml"), format!("[workspace]\nmembers=['plugins/*']\nresolver='3'\n[workspace.dependencies]\ncordis-sdk={{path={sdk}}}\nkv-api={{path={api}}}\nkv-mem-component={{path={memory}}}\ncounter-component={{path={counter}}}\n")).unwrap();
    for (name, component, source) in [
        (
            "memory",
            "kv-mem-component",
            include_str!("../../../plugins/kv-mem/src/lib.rs"),
        ),
        (
            "counter",
            "counter-component",
            include_str!("../../../plugins/counter/src/lib.rs"),
        ),
    ] {
        let directory = project.join("plugins").join(name);
        fs::create_dir_all(directory.join("src")).unwrap();
        fs::write(directory.join("src/lib.rs"), source).unwrap();
        fs::write(directory.join("Cargo.toml"), format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2024'\n[lib]\ncrate-type=['cdylib']\n[dependencies]\ncordis-sdk.workspace=true\n{component}.workspace=true\n")).unwrap();
    }
    let extension = std::env::consts::DLL_SUFFIX;
    fs::write(project.join("cordis.toml"), format!("requires_core='>=0.1.0,<0.2.0'\n[pack]\nname='pack-native'\nversion='0.1.0'\n[runtime]\nplugin_dir='target/debug'\ncheckpoint_ms=0\n[instance.store]\nplugin='libmemory{extension}'\ncomponent='kv-store'\ngroup='kv'\n[instance.counter]\nplugin='libcounter{extension}'\ncomponent='counter'\ngroup='counter'\n")).unwrap();
    (root, project)
}

/// AU-12：解开的发布目录保留运行程序，源码与开发工具保留在原项目。
fn release(project: &Path, runtime_dir: &Path, sign: bool) -> PathBuf {
    let _build = cordis_testkit::build::shared_build_lock(&tools().root).unwrap();
    let target = if cfg!(target_os = "macos") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let mut pack = Command::new(tools().binaries.join("cordis"));
    pack.arg("pack")
        .arg("--project")
        .arg(project)
        .args(["--target", target])
        .arg("--target-dir")
        .arg(cordis_testkit::build::shared_target_dir(&tools().root))
        .arg("--runtime-dir")
        .arg(runtime_dir);
    if sign {
        pack.args(["--sign=-", "--allow-third-party"]);
    }
    command(&mut pack);
    let name = format!("pack-native-0.1.0-{target}");
    let archive = fs::File::open(project.join("dist").join(format!("{name}.tar.gz"))).unwrap();
    let extracted = project.join("delivered");
    fs::create_dir(&extracted).unwrap();
    tar::Archive::new(flate2::read::GzDecoder::new(archive))
        .unpack(&extracted)
        .unwrap();
    let deployed = extracted.join(name);
    assert!(
        !deployed.join("cordis-dev").exists(),
        "发布包由运行程序组成"
    );
    assert!(
        !deployed.join("Cargo.toml").exists(),
        "发布目录独立于项目源码"
    );
    deployed
}

/// AU-12：PATH 只指向发布目录，业务实例从已打包的真实程序进入 Active。
fn activate(project: &Path, deployed: &Path, count: usize) -> Value {
    activate_at(
        project,
        &deployed.join("cordis"),
        deployed,
        &deployed.join("state"),
        count,
    )
}

fn activate_at(
    project: &Path,
    executable: &Path,
    directory: &Path,
    state: &Path,
    count: usize,
) -> Value {
    let log = fs::File::create(project.join("core.log")).unwrap();
    let child = Command::new(executable)
        .current_dir(directory)
        .env_clear()
        .env("PATH", executable.parent().unwrap())
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let mut core = Core(child);
    let started = Instant::now();
    let snapshot = loop {
        if let Ok(bytes) = fs::read(state.join("status.json")) {
            let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
            if snapshot["instances"].as_array().is_some_and(|instances| {
                instances.len() == count
                    && instances
                        .iter()
                        .all(|instance| instance["state"] == "active")
            }) {
                break snapshot;
            }
        }
        assert!(
            core.0.try_wait().unwrap().is_none(),
            "核心提前退出：{}",
            fs::read_to_string(project.join("core.log")).unwrap()
        );
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "实例进入 Active 的验收截止时间"
        );
        thread::sleep(Duration::from_millis(20));
    };
    fs::write(
        project.join("accepted.json"),
        serde_json::to_vec_pretty(&snapshot).unwrap(),
    )
    .unwrap();
    command(Command::new("/bin/kill").args(["-TERM", &core.0.id().to_string()]));
    assert!(core.0.wait().unwrap().success());
    snapshot
}

#[test]
fn archive_runs_real_plugins_without_toolchain_or_source() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "pack-native");
    let _tools = tools().for_case(&case);
    let (_, project) = project("native");
    let project = case.track(project);
    let deployed = release(&project, &tools().binaries, false);
    activate(&project, &deployed, 2);
    eprintln!(
        "AU-12 真实压缩包运行：{}",
        case.evidence_for(&project).display()
    );
    case.success();
}

/// DOC-15 M16：实际 Rust UP/1 进程验证运行时目录与命令改写。
#[test]
fn bundled_runtime_executes_real_process_plugin() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "pack-runtime");
    let lease = tools().for_case(&case);
    let root = &lease.root;
    let project = case.track(
        root.join("target/pack-tests")
            .join(format!("runtime-{}", std::process::id())),
    );
    let provider = project.join("plugins/provider");
    let runtime = project.join("provided-runtime/bin");
    fs::create_dir_all(&provider).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    let build_lock = cordis_testkit::build::shared_build_lock(root).unwrap();
    let target = cordis_testkit::build::shared_target_dir(root);
    command(
        Command::new(env!("CARGO"))
            .args(["build", "--release", "--manifest-path"])
            .arg(root.join("crates/cordis-runtime/fixtures/plugin/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target),
    );
    fs::copy(
        target.join("release/cordis-runtime-fixture"),
        runtime.join("python3"),
    )
    .unwrap();
    drop(build_lock);
    fs::write(provider.join("cordis-plugin.toml"), "name='fixture-provider'\nversion='0.1.0'\nsocket='process'\nprotocol=1\ncommand=['python3','provider','v1']\n").unwrap();
    fs::write(project.join("cordis.toml"), "[pack]\nname='pack-native'\nversion='0.1.0'\nruntimes=['python']\n[pack.runtime_paths]\npython='provided-runtime'\n[runtime]\ncheckpoint_ms=0\n[instance.provider]\nplugin='provider'\ncomponent='provider'\nconfig={initial=41}\n").unwrap();
    let deployed = release(&project, &tools().binaries, false);
    let manifest: toml::Table = toml::from_str(
        &fs::read_to_string(deployed.join("plugins/plugin-1/cordis-plugin.toml")).unwrap(),
    )
    .unwrap();
    let command = manifest["command"][0].as_str().unwrap();
    assert_eq!(
        deployed
            .join("plugins/plugin-1")
            .join(command)
            .canonicalize()
            .unwrap(),
        deployed
            .join("runtimes/python/bin/python3")
            .canonicalize()
            .unwrap()
    );
    activate(&project, &deployed, 1);
    eprintln!(
        "M16 真实进程与运行时目录：{}",
        case.evidence_for(&project).display()
    );
    case.success();
}

/// ADR-0036：真实源印记在签名前建立，发布时在签名后的库上重新运行契约。
#[cfg(target_os = "macos")]
fn verified_project(root: &Path, project: &Path) -> PathBuf {
    let contracts = project.join("contracts/kv");
    fs::create_dir_all(contracts.join("src")).unwrap();
    fs::write(
        contracts.join("src/lib.rs"),
        include_str!("../../../plugins/kv-contract/src/lib.rs"),
    )
    .unwrap();
    fs::write(contracts.join("Cargo.toml"), "[package]\nname='kv-contract'\nversion='0.1.0'\nedition='2024'\n[lib]\ncrate-type=['cdylib']\n[dependencies]\ncordis-sdk.workspace=true\ncordis-contract.workspace=true\nkv-api.workspace=true\n").unwrap();
    let mut workspace: toml::Table =
        toml::from_str(&fs::read_to_string(project.join("Cargo.toml")).unwrap()).unwrap();
    workspace["workspace"]["members"]
        .as_array_mut()
        .unwrap()
        .push(toml::Value::String("contracts/*".into()));
    let dependency = toml::Table::from_iter([(
        "path".into(),
        toml::Value::String(
            root.join("crates/cordis-contract")
                .to_string_lossy()
                .into_owned(),
        ),
    )]);
    workspace["workspace"]["dependencies"]
        .as_table_mut()
        .unwrap()
        .insert("cordis-contract".into(), toml::Value::Table(dependency));
    fs::write(
        project.join("Cargo.toml"),
        toml::to_string(&workspace).unwrap(),
    )
    .unwrap();
    let mut configuration: toml::Table =
        toml::from_str(&fs::read_to_string(project.join("cordis.toml")).unwrap()).unwrap();
    configuration["runtime"]
        .as_table_mut()
        .unwrap()
        .insert("require_verified".into(), toml::Value::Boolean(true));
    configuration["instance"]
        .as_table_mut()
        .unwrap()
        .remove("counter");
    fs::write(
        project.join("cordis.toml"),
        toml::to_string(&configuration).unwrap(),
    )
    .unwrap();
    let build_lock = cordis_testkit::build::shared_build_lock(root).unwrap();
    let shared = cordis_testkit::build::shared_target_dir(root);
    command(
        Command::new(env!("CARGO"))
            .current_dir(project)
            .args([
                "build",
                "--release",
                "--workspace",
                "--target",
                "aarch64-apple-darwin",
                "--target-dir",
            ])
            .arg(&shared),
    );
    let directory = project.join("target/aarch64-apple-darwin/release");
    fs::create_dir_all(&directory).unwrap();
    for name in [
        "libmemory.dylib",
        "libcounter.dylib",
        "libkv_contract.dylib",
    ] {
        fs::copy(
            shared.join("aarch64-apple-darwin/release").join(name),
            directory.join(name),
        )
        .unwrap();
    }
    drop(build_lock);
    command(
        Command::new(tools().binaries.join("cordis-dev"))
            .current_dir(project)
            .arg("verify")
            .arg("--project")
            .arg(project)
            .arg("--plugin")
            .arg(directory.join("libmemory.dylib"))
            .arg("--suite")
            .arg(directory.join("libkv_contract.dylib")),
    );
    directory.join("libmemory.dylib")
}

#[cfg(target_os = "macos")]
#[test]
fn signed_archive_revalidates_and_moves_verified_libraries() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "pack-signed");
    let _tools = self::tools().for_case(&case);
    let (root, project) = project("signed");
    let project = case.track(project);
    let source = verified_project(&root, &project);
    let original = fs::read(source).unwrap();
    let tools = project.join("tools");
    fs::create_dir(&tools).unwrap();
    for name in ["cordis", "cordis-dev"] {
        fs::copy(self::tools().binaries.join(name), tools.join(name)).unwrap();
    }
    fs::copy(
        self::tools().binaries.join("cordis-host"),
        tools.join("cordis-host"),
    )
    .unwrap();
    let deployed = release(&project, &tools, true);
    let library = deployed.join("plugins/plugin-1/libmemory.dylib");
    assert_ne!(
        original,
        fs::read(&library).unwrap(),
        "hardened runtime 签名改变原始验证字节"
    );
    let snapshot = activate(&project, &deployed, 1);
    assert!(
        snapshot["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|library| library["name"] == "kv" && library["verified"] == true),
        "移动后的实际 Host 验证通过"
    );
    eprintln!(
        "M16 签名、真实重验与印记移动：{}",
        case.evidence_for(&project).display()
    );
    case.success();
}

#[cfg(target_os = "macos")]
#[test]
fn application_keeps_signed_resources_and_selects_multiple_verified_roots() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "pack-app");
    let _tools = self::tools().for_case(&case);
    let (root, project) = project("app");
    let project = case.track(project);
    verified_project(&root, &project);
    let state = project.join("app-state");
    let mut configuration: toml::Table =
        toml::from_str(&fs::read_to_string(project.join("cordis.toml")).unwrap()).unwrap();
    configuration["pack"].as_table_mut().unwrap().insert(
        "state_dir".into(),
        toml::Value::String(state.to_string_lossy().into_owned()),
    );
    fs::write(
        project.join("cordis.toml"),
        toml::to_string(&configuration).unwrap(),
    )
    .unwrap();
    let tools = project.join("tools");
    fs::create_dir(&tools).unwrap();
    fs::copy(self::tools().binaries.join("cordis"), tools.join("cordis")).unwrap();
    fs::copy(
        self::tools().binaries.join("cordis-host"),
        tools.join("cordis-host"),
    )
    .unwrap();
    fs::copy(
        self::tools().binaries.join("cordis-dev"),
        tools.join("cordis-dev"),
    )
    .unwrap();
    let build_lock = cordis_testkit::build::shared_build_lock(&root).unwrap();
    command(
        Command::new(self::tools().binaries.join("cordis"))
            .arg("pack")
            .arg("--project")
            .arg(&project)
            .args(["--target", "aarch64-apple-darwin", "--app"])
            .arg("--target-dir")
            .arg(cordis_testkit::build::shared_target_dir(&root))
            .arg("--runtime-dir")
            .arg(&tools),
    );
    drop(build_lock);
    let app = project.join("dist/pack-native.app");
    let launch = project.join("launch");
    fs::create_dir(&launch).unwrap();
    let snapshot = activate_at(
        &project,
        &app.join("Contents/MacOS/cordis"),
        &launch,
        &state,
        1,
    );
    assert!(
        snapshot["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|library| library["name"] == "kv" && library["verified"] == true)
    );
    multiple_roots(&project, &app, &tools.join("cordis-host"));
    command(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(&app),
    );
    eprintln!(
        "M16 app 签名资源与多验证根：{}",
        case.evidence_for(&project).display()
    );
    case.success();
}

/// ADR-0036：先遇到另一真实 DLL 和缺席目录，再由正确根完成实际 dlopen。
#[cfg(target_os = "macos")]
fn multiple_roots(project: &Path, app: &Path, host: &Path) {
    let resources = app.join("Contents/Resources");
    let plugin = resources.join("plugins/plugin-1/libmemory.dylib");
    let receipt: Value = serde_json::from_slice(
        &fs::read(resources.join("plugins/plugin-1/libmemory.dylib.verified.json")).unwrap(),
    )
    .unwrap();
    let wrong = project.join("wrong-verified");
    fs::create_dir(&wrong).unwrap();
    fs::copy(
        project.join("target/aarch64-apple-darwin/release/libcounter.dylib"),
        wrong.join(receipt["verified_copy"].as_str().unwrap()),
    )
    .unwrap();
    let output = Command::new(host)
        .arg("--describe")
        .arg(plugin)
        .arg("--cache-dir")
        .arg(project.join("probe-cache"))
        .arg("--require-verified")
        .arg("--verified-dir")
        .arg(wrong)
        .arg("--verified-dir")
        .arg(project.join("missing-verified"))
        .arg("--verified-dir")
        .arg(resources.join(".cordis/verified"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["name"], "kv");
}

/// DOC-15 §12.2：显式发布的静态目录保留路径和实际内容。
#[test]
fn declared_static_assets_are_delivered_at_their_configuration_path() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "pack-assets");
    let lease = tools().for_case(&case);
    let root = &lease.root;
    let project = case.track(
        root.join("target/pack-tests")
            .join(format!("assets-{}", std::process::id())),
    );
    fs::create_dir_all(project.join("plugins")).unwrap();
    fs::create_dir(project.join("site")).unwrap();
    fs::copy(
        tools().binaries.join(format!(
            "{}kv_mem{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        )),
        project
            .join("plugins")
            .join(format!("libmemory{}", std::env::consts::DLL_SUFFIX)),
    )
    .unwrap();
    fs::write(
        project.join("site/index.html"),
        "<h1>Cordis release assets</h1>\n",
    )
    .unwrap();
    fs::write(project.join("cordis.toml"), format!("[pack]\nname='pack-native'\nversion='0.1.0'\nassets=['site']\n[instance.store]\nplugin='libmemory{}'\ncomponent='kv-store'\n", std::env::consts::DLL_SUFFIX)).unwrap();
    let deployed = release(&project, &tools().binaries, false);
    assert_eq!(
        fs::read_to_string(deployed.join("site/index.html")).unwrap(),
        "<h1>Cordis release assets</h1>\n"
    );
    eprintln!(
        "M16 静态发布资源：{}",
        case.evidence_for(&project).display()
    );
    case.success();
}
