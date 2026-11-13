//! AC-1、AU-11：验收进程与工作目录的显式拥有者，失败保留运行证据。

use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub struct Tools {
    pub root: PathBuf,
    pub core: PathBuf,
    pub dev: PathBuf,
    pub host: PathBuf,
}

pub fn command(command: &mut Command) -> Output {
    let output = command.output().expect("运行验收命令");
    assert!(
        output.status.success(),
        "命令失败 {command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Y-3：测试自身构建运行程序，所有场景共享同一组已编译的最终源码产物。
pub fn tools() -> &'static Tools {
    static TOOLS: OnceLock<Tools> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        if let (Ok(core), Ok(dev), Ok(host)) = (
            env::var("CORDIS_CORE_BIN"),
            env::var("CORDIS_DEV_BIN"),
            env::var("CORDIS_HOST_BIN"),
        ) {
            return Tools {
                root,
                core: core.into(),
                dev: dev.into(),
                host: host.into(),
            };
        }
        let _build = cordis_testkit::build::shared_build_lock(&root).unwrap();
        let build = cordis_testkit::build::shared_target_dir(&root);
        command(
            Command::new("cargo")
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
                    "--target-dir",
                ])
                .arg(&build),
        );
        let copied = cordis_testkit::build::snapshot_artifacts(
            &root,
            "acceptance-tools",
            "release",
            &["cordis", "cordis-dev", "cordis-host"],
        )
        .unwrap();
        Tools {
            root,
            core: copied.join("cordis"),
            dev: copied.join("cordis-dev"),
            host: copied.join("cordis-host"),
        }
    })
}

impl Tools {
    pub fn project(&self, label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let project = self
            .root
            .join("target/acceptance-evidence")
            .join(format!("{label}-{}-{unique}", std::process::id()));
        fs::create_dir_all(project.parent().unwrap()).unwrap();
        project
    }

    pub fn path(&self) -> std::ffi::OsString {
        let mut paths = vec![
            self.dev.parent().unwrap().to_owned(),
            self.host.parent().unwrap().to_owned(),
        ];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        env::join_paths(paths).unwrap()
    }

    pub fn generate(&self, project: &Path) {
        command(
            Command::new(&self.core)
                .env("PATH", self.path())
                .arg("new")
                .arg(project)
                .arg("--sdk-path")
                .arg(self.root.join("crates/cordis-sdk")),
        );
    }

    pub fn plugin(&self, project: &Path, name: &str) {
        command(
            Command::new(&self.core)
                .env("PATH", self.path())
                .args(["new-plugin", name, "--rust", "--project"])
                .arg(project),
        );
    }

    pub fn start(&self, project: &Path) -> Running {
        Running(
            Command::new(&self.core)
                .current_dir(project)
                .env("PATH", self.path())
                .arg("run")
                .stdin(Stdio::null())
                .stdout(fs::File::create(project.join("runtime.stdout.log")).unwrap())
                .stderr(fs::File::create(project.join("runtime.stderr.log")).unwrap())
                .spawn()
                .unwrap(),
        )
    }
}

pub struct Running(pub Child);
impl Running {
    pub fn stop(&mut self) {
        command(Command::new("/bin/kill").args(["-TERM", &self.0.id().to_string()]));
        let status = until("核心有序退出", || self.0.try_wait().unwrap());
        assert!(status.success(), "核心有序退出失败 {status}");
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub fn status(project: &Path) -> Option<Value> {
    fs::read(project.join("state/status.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// AC-1：30 秒作为既定验收上限，观察最终状态而非固定等待时间。
pub fn until<T>(condition: &str, mut observe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = observe() {
            return value;
        }
        assert!(Instant::now() < deadline, "等待{condition}超时");
        thread::sleep(Duration::from_millis(20));
    }
}

pub fn active(snapshot: &Value, count: usize) -> bool {
    snapshot["instances"].as_array().is_some_and(|instances| {
        instances.len() == count
            && instances
                .iter()
                .all(|instance| instance["state"] == "active")
    })
}

pub fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
