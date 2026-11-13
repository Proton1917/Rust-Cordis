//! DOC-11 §3.2、DOC-12 §7.3：真实旧核心、旧 watch 与新增快照字段的兼容性。
#![cfg(target_os = "macos")]

use cordis_testkit::{
    build::{shared_build_lock, shared_target_dir, snapshot_artifacts},
    cleanup::TestGuard,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

fn command(command: &mut Command) {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

struct Core(Child);
impl Core {
    fn start(binary: &Path, project: &Path) -> Self {
        let log = fs::File::create(project.join("core.log")).unwrap();
        fs::copy(binary, project.join("cordis")).unwrap();
        Self(
            Command::new(project.join("cordis"))
                .arg("run")
                .current_dir(project)
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
        )
    }
    fn stop(&mut self) {
        command(Command::new("kill").args(["-TERM", &self.0.id().to_string()]));
        assert!(self.0.wait().unwrap().success());
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}

fn wait(project: &Path, core: &mut Core, ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(core.0.try_wait().unwrap().is_none());
        if let Ok(bytes) = fs::read(project.join("state/status.json")) {
            let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
            if ready(&snapshot) {
                return snapshot;
            }
        }
        assert!(Instant::now() < deadline, "{}", project.display());
        thread::sleep(Duration::from_millis(20));
    }
}

fn legacy(root: &Path, directory: &Path) -> PathBuf {
    let cached = root.join("target/releases/v0.2.0/cordis-v0.2.0-aarch64-apple-darwin.tar.gz");
    let archive = if cached.is_file() {
        cached
    } else {
        let path = directory.join("v0.2.0.tar.gz");
        command(Command::new("curl").args(["--fail", "--location", "--silent", "--show-error", "https://github.com/Proton1917/Rust-Cordis/releases/download/v0.2.0/cordis-v0.2.0-aarch64-apple-darwin.tar.gz", "--output"]).arg(&path));
        path
    };
    command(
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(directory),
    );
    directory.join("cordis-v0.2.0-aarch64-apple-darwin")
}

fn project(directory: &Path, label: &str, host: &Path, watch: &Path) -> PathBuf {
    let path = directory.join(label);
    fs::create_dir_all(path.join("plugins")).unwrap();
    fs::create_dir(path.join("candidates")).unwrap();
    fs::copy(host, path.join("cordis-host")).unwrap();
    fs::copy(watch, path.join("plugins/libwatch.dylib")).unwrap();
    fs::write(path.join("cordis.toml"), "[runtime]\ncheckpoint_ms=0\nconfirm_ms=20\n[instance.watch]\nplugin='libwatch.dylib'\ncomponent='watch'\ngrants={'cordis.manage@1'=['install','replace_batch']}\nconfig={paths=['candidates'],debounce_ms=100}\n").unwrap();
    path
}

#[test]
fn runtime_14_preserves_13_readers_and_watch_declares_its_minimum() {
    let case = TestGuard::for_package(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "watch-runtime-compat",
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let directory = case.directory("watch-compat");
    fs::create_dir_all(&directory).unwrap();
    let old = legacy(&root, &directory);
    let built = build_watch(&root, &case);
    let pending = new_watch_old_core(&directory, &old, &built);
    let (active, replaced) = old_watch_new_core(&directory, &old, &built);
    fs::write(directory.join("result.json"), serde_json::to_vec_pretty(&json!({"new_watch_old_core":pending,"old_row_page_of":null,"old_watch_new_core":active,"self_replacement":replaced})).unwrap()).unwrap();
    eprintln!(
        "watch 版本兼容证据：{}",
        case.evidence_for(&directory).display()
    );
    case.success();
}

fn build_watch(root: &Path, case: &TestGuard) -> PathBuf {
    let target = shared_target_dir(root);
    {
        let _lock = shared_build_lock(root).unwrap();
        command(
            Command::new(env!("CARGO"))
                .current_dir(root)
                .args([
                    "build",
                    "-p",
                    "cordis-host",
                    "-p",
                    "cordis-watch",
                    "--target-dir",
                ])
                .arg(&target),
        );
        snapshot_artifacts(
            case,
            "watch-compat",
            "debug",
            &["cordis-host", "libcordis_watch.dylib"],
        )
        .unwrap()
    }
}

fn new_watch_old_core(directory: &Path, old: &Path, built: &Path) -> Value {
    let previous = project(
        directory,
        "old-core",
        &old.join("bin/cordis-host"),
        &built.join("libcordis_watch.dylib"),
    );
    let mut core = Core::start(&old.join("bin/cordis"), &previous);
    let pending = wait(&previous, &mut core, |state| {
        state["groups"]
            .as_array()
            .is_some_and(|groups| groups.iter().any(|group| group["state"] == "running"))
            && state["instances"][0]["state"] == "pending"
    });
    assert!(
        serde_json::to_string(&pending)
            .unwrap()
            .contains("cordis.runtime@1.4")
    );
    let row: cordis_types::InstanceInfo =
        serde_json::from_value(pending["instances"][0].clone()).unwrap();
    assert!(row.page_of.is_none());
    fs::write(
        previous.join("observed.json"),
        serde_json::to_vec_pretty(&pending).unwrap(),
    )
    .unwrap();
    core.stop();
    pending
}

fn old_watch_new_core(directory: &Path, old: &Path, built: &Path) -> (Value, Value) {
    let current = project(
        directory,
        "old-watch",
        &built.join("cordis-host"),
        &old.join("plugins/libcordis_watch.dylib"),
    );
    let mut core = Core::start(Path::new(env!("CARGO_BIN_EXE_cordis")), &current);
    let active = wait(&current, &mut core, |state| {
        state["instances"][0]["state"] == "active"
    });
    assert!(
        serde_json::to_string(&active)
            .unwrap()
            .contains("cordis.runtime@1.3")
    );
    fs::copy(
        built.join("libcordis_watch.dylib"),
        current.join("candidates/next.pending"),
    )
    .unwrap();
    fs::rename(
        current.join("candidates/next.pending"),
        current.join("candidates/libwatch.dylib"),
    )
    .unwrap();
    let replaced = wait(&current, &mut core, |state| {
        state["last_batch"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item["caller"] == "watch" && item["state"] == "succeeded")
        }) && state["instances"][0]["state"] == "active"
    });
    core.stop();
    (active, replaced)
}
