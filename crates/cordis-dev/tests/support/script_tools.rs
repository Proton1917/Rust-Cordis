//! DOC-15 M18/M19：脚本 SDK 验收共用当前源码的运行程序和原生插件。

use cordis_testkit::cleanup::{FixtureLease, SharedFixture};
use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
use std::{
    process::Child,
    thread,
    time::{Duration, Instant},
};

pub struct Running(pub Child);
impl Drop for Running {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub fn active(project: &Path, core: &mut Running, previous: Option<u64>, marker: &str) -> Value {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            core.0.try_wait().unwrap().is_none(),
            "核心提前退出：{}",
            project.display()
        );
        if let Ok(bytes) = fs::read(project.join("state/status.json"))
            && let Ok(snapshot) = serde_json::from_slice::<Value>(&bytes)
            && let Some(instances) = snapshot["instances"].as_array()
            && instances.len() == 3
            && instances
                .iter()
                .all(|instance| instance["state"] == "active")
            && let Some(hello) = instances
                .iter()
                .find(|instance| instance["name"] == "hello")
            && previous.is_none_or(|old| hello["context"].as_u64() != Some(old))
            && (previous.is_none()
                || fs::read_to_string(project.join("core.log"))
                    .unwrap()
                    .contains(marker))
        {
            return hello.clone();
        }
        assert!(
            Instant::now() < until,
            "等待当前脚本版本激活：{}",
            project.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

pub struct Tools {
    pub root: PathBuf,
    pub binaries: PathBuf,
}

pub fn checked(command: &mut Command) {
    let result = command.output().expect("执行脚本插件验收命令");
    assert!(
        result.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

/// Y-3、项目规则 11：统一构建并复制当前源码的运行程序、插件与管理客户端。
pub fn tools() -> FixtureLease<Tools> {
    static TOOLS: SharedFixture<Tools> = SharedFixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    TOOLS.get_or_init(&root, "script-sdk-tools", |guard| {
        let _lock = cordis_testkit::build::shared_build_lock(&root).unwrap();
        let target = cordis_testkit::build::shared_target_dir(&root);
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
                    "counter",
                    "-p",
                    "kv-contract",
                    "--target-dir",
                ])
                .arg(&target),
        );
        checked(
            Command::new(env!("CARGO"))
                .current_dir(&root)
                .args(["build", "--manifest-path"])
                .arg(root.join("crates/cordis-runtime/fixtures/plugin/Cargo.toml"))
                .arg("--target-dir")
                .arg(&target),
        );
        let libraries = ["kv_mem", "counter", "kv_contract"].map(|name| {
            format!(
                "{}{name}{}",
                env::consts::DLL_PREFIX,
                env::consts::DLL_SUFFIX
            )
        });
        let binaries = cordis_testkit::build::snapshot_artifacts(
            guard,
            "script-sdk-tools",
            "debug",
            &[
                "cordis",
                "cordis-dev",
                "cordis-host",
                "cordis-runtime-fixture",
                &libraries[0],
                &libraries[1],
                &libraries[2],
            ],
        )
        .unwrap();
        Tools {
            root: root.clone(),
            binaries,
        }
    })
}
