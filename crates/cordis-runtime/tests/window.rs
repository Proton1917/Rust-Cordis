//! DOC-15 §11.6、AU-17：发布应用中的窗口调用、单次进程恢复与核心有序退出。
#![cfg(target_os = "macos")]

use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn command(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Y-3、AU-17：当前源码自行构建工具和插件，产物目录显式位于项目 target。
fn tools(root: &Path) -> PathBuf {
    let target = root.join("target/pack-tools");
    let mut build = Command::new(env!("CARGO"));
    build.current_dir(root).args(["build", "--release"]);
    for package in [
        "cordis-runtime",
        "cordis-dev",
        "cordis-host",
        "cordis-window",
        "cordis-http",
        "kv-mem",
    ] {
        build.args(["-p", package]);
    }
    eprintln!("AU-17：构建正式核心、工具和窗口插件");
    command(build.arg("--target-dir").arg(&target));
    command(
        Command::new(env!("CARGO"))
            .args(["build", "--release", "--manifest-path"])
            .arg(root.join("crates/cordis-runtime/fixtures/window-probe/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target),
    );
    target.join("release")
}

/// ADR-0036：正式 pack 接收真实运行产物，应用状态明确存放在本次场景目录。
fn deployment(root: &Path, binaries: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let project = root
        .join("target/window-tests")
        .join(format!("recovery-{}", std::process::id()));
    let plugins = project.join("plugins");
    let window = plugins.join("window");
    fs::create_dir_all(&window).unwrap();
    for name in [
        "libcordis_http.dylib",
        "libwindow_probe.dylib",
        "libkv_mem.dylib",
    ] {
        fs::copy(binaries.join(name), plugins.join(name)).unwrap();
    }
    fs::copy(binaries.join("cordis-window"), window.join("cordis-window")).unwrap();
    fs::copy(
        root.join("plugins/window/cordis-plugin.toml"),
        window.join("cordis-plugin.toml"),
    )
    .unwrap();
    let state = project.join("app-state");
    let quoted = toml::Value::String(state.to_string_lossy().into_owned());
    fs::write(
        project.join("cordis.toml"),
        format!(
            "requires_core='>=0.1.0,<0.2.0'\n[pack]\nname='window-recovery'\nstate_dir={quoted}\n\n[runtime]\ncheckpoint_ms=250\nrestart_backoff_ms=100\n\n[instance.http]\nplugin='libcordis_http.dylib'\ncomponent='http'\ngroup='http'\n\n[instance.window]\nplugin='window'\ncomponent='window'\ngroup='window'\ngrants={{'cordis.manage@1'=['shutdown']}}\nconfig={{title='Cordis 窗口验收',width=960,height=680,quit_on_last_window=true}}\n\n[instance.probe]\nplugin='libwindow_probe.dylib'\ncomponent='window-probe'\ngroup='probe'\nconfig={{url='about:blank'}}\n\n[instance.store]\nplugin='libkv_mem.dylib'\ncomponent='kv-store'\ngroup='kv'\n"
        ),
    )
    .unwrap();
    command(
        Command::new(binaries.join("cordis"))
            .arg("pack")
            .arg("--project")
            .arg(&project)
            .args(["--target", "aarch64-apple-darwin", "--app"])
            .arg("--runtime-dir")
            .arg(binaries),
    );
    let app = project.join("dist/window-recovery.app");
    (project, state, app)
}

struct Core(Child);

impl Drop for Core {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// AU-17：从 Bundle 外启动默认入口，核心和所有插件均由本次 Child 拥有。
fn launch(project: &Path, app: &Path) -> Core {
    let directory = project.join("launch");
    fs::create_dir(&directory).unwrap();
    let log = fs::File::create(project.join("core.log")).unwrap();
    Core(
        Command::new(app.join("Contents/MacOS/cordis"))
            .current_dir(directory)
            .env("PATH", app.join("Contents/MacOS"))
            .stdout(log.try_clone().unwrap())
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    )
}

/// AU-17：固定三十秒截止时间，观察目标条件以处理异步状态发布。
fn until<T>(label: &str, mut observe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = observe() {
            return value;
        }
        assert!(Instant::now() < deadline, "等待{label}超过三十秒");
        thread::sleep(Duration::from_millis(20));
    }
}

fn instance<'a>(snapshot: &'a Value, name: &str) -> &'a Value {
    snapshot["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|instance| instance["name"] == name)
        .unwrap()
}

fn group<'a>(snapshot: &'a Value, name: &str) -> &'a Value {
    snapshot["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["name"] == name)
        .unwrap()
}

fn active(core: &mut Core, state: &Path) -> Option<Value> {
    assert!(core.0.try_wait().unwrap().is_none(), "窗口验收期间核心退出");
    let snapshot: Value =
        serde_json::from_slice(&fs::read(state.join("status.json")).ok()?).ok()?;
    (snapshot["pid"] == core.0.id()
        && snapshot["instances"].as_array()?.len() == 4
        && snapshot["instances"]
            .as_array()?
            .iter()
            .all(|instance| instance["state"] == "active"))
    .then_some(snapshot)
}

fn observation(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    cordis_up::decode_value(&bytes, &cordis_up::Limits::DEFAULT)
        .ok()?
        .deserialized()
        .ok()
}

/// AU-17：按真实 apply 标记配对窗口调用与事件，恢复时等待新的激活标记。
fn observed_window(state: &Path, previous: Option<&Value>) -> Value {
    let directory = state.join("data/window-probe");
    until(
        "当前 probe 的窗口调用与 closed 事件配对",
        || {
            let called = observation(&directory.join("called.cbor"))?;
            let closed = observation(&directory.join("closed.cbor"))?;
            (called == closed
                && previous.is_none_or(|previous| called["activation"] != previous["activation"]))
            .then_some(called)
        },
    )
}

/// AU-17：只对当前窗口 PID 注入一次 SIGKILL，等待窗口和真实依赖方的新纪元。
fn recover(core: &mut Core, state: &Path, before: &Value) -> (Value, u128) {
    let old_window = group(before, "window")["pid"].as_u64().unwrap();
    let started = Instant::now();
    command(Command::new("/bin/kill").args(["-KILL", &old_window.to_string()]));
    let after = until("窗口进程与 probe 重新激活", || {
        active(core, state).filter(|snapshot| {
            group(snapshot, "window")["pid"] != old_window
                && instance(snapshot, "window")["context"] != instance(before, "window")["context"]
                && instance(snapshot, "probe")["context"] != instance(before, "probe")["context"]
        })
    });
    let recovery_ms = started.elapsed().as_millis();
    assert_eq!(after["pid"], before["pid"], "窗口恢复期间核心 PID 保持");
    for name in ["http", "store"] {
        assert_eq!(
            instance(&after, name)["context"],
            instance(before, name)["context"],
            "窗口恢复期间 {name} 纪元保持"
        );
    }
    assert_eq!(group(&after, "window")["restarts"], 1);
    (after, recovery_ms)
}

fn save(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

/// AU-17：SIGTERM 触发有序退出，随后检查本次实际核心与插件 PID 均已结束。
fn stop(core: &mut Core, before: &Value, after: &Value) {
    let mut pids = BTreeSet::from([u64::from(core.0.id())]);
    for snapshot in [before, after] {
        pids.extend(
            snapshot["groups"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|group| group["pid"].as_u64()),
        );
    }
    command(Command::new("/bin/kill").args(["-TERM", &core.0.id().to_string()]));
    let exited = until("核心有序退出", || core.0.try_wait().unwrap());
    assert!(exited.success(), "核心有序退出状态 {exited}");
    until("本次核心和插件进程释放", || {
        pids.iter()
            .all(|pid| {
                !Command::new("/bin/kill")
                    .args(["-0", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            })
            .then_some(())
    });
}

#[test]
fn packed_window_recovers_once_with_backend_contexts_and_signature_preserved() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let binaries = tools(&root);
    let (project, state, app) = deployment(&root, &binaries);
    eprintln!("AU-17：运行发布应用 {}", app.display());
    let mut core = launch(&project, &app);
    let before = until("四个实例 Active", || active(&mut core, &state));
    let first_window = observed_window(&state, None);
    save(&project.join("before-crash.json"), &before);
    let (after, recovery_ms) = recover(&mut core, &state, &before);
    let restored_window = observed_window(&state, Some(&first_window));
    save(&project.join("after-crash.json"), &after);
    stop(&mut core, &before, &after);
    command(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(&app),
    );
    save(
        &project.join("result.json"),
        &json!({
            "scenario":"AU-17 process recovery", "core_pid":before["pid"],
            "previous_window_pid":group(&before,"window")["pid"],
            "restored_window_pid":group(&after,"window")["pid"],
            "recovery_ms":recovery_ms, "restart_count":1,
            "first_probe":first_window, "restored_probe":restored_window,
            "exit_code":0, "remaining_processes":[], "app_signature":"valid"
        }),
    );
    eprintln!("AU-17 真实窗口进程验收：{}", project.display());
}
