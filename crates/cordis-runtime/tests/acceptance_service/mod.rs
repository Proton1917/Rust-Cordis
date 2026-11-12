//! DOC-15 AU-11、RM-10：项目内临时 launchd job 在核心退出后恢复状态。

use crate::support::{active, command, status, tools, until, write_json};
use serde_json::{Value as JsonValue, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
use toml::{Table, Value};

struct Job {
    target: String,
    active: bool,
}
impl Job {
    fn stop(&mut self) {
        command(Command::new("launchctl").args(["bootout", &self.target]));
        self.active = false;
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        if self.active {
            let output = Command::new("launchctl")
                .args(["bootout", &self.target])
                .output();
            if let Ok(output) = output
                && !output.status.success()
            {
                eprintln!(
                    "清理临时服务 {}：{}",
                    self.target,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

pub fn recover() {
    let project = prepare_deployment();
    let mut job = launch(&project);
    let initial = until("服务管理器启动核心", || {
        status(&project).filter(|snapshot| active(snapshot, 1))
    });
    let (restored, recovery_ms) = restart_from_checkpoint(&project, &initial);
    let restored_core = restored["pid"].as_u64().unwrap();
    let restored_hosts = host_pids(&restored);
    job.stop();
    until("临时服务进程回收", || {
        (!exists(restored_core) && restored_hosts.iter().all(|pid| !exists(*pid))).then_some(())
    });
    write_json(
        &project.join("result.json"),
        &json!({"scenario":"AU-11", "RM-10_ms":recovery_ms,"previous_core":initial["pid"],"restored_core":restored_core,"previous_plugins":host_pids(&initial),"restored_plugins":restored_hosts,"inherited_value":73,"new_initial_value":29,"launchd_target":job.target,"service_removed":true}),
    );
    eprintln!(
        "AU-11 / RM-10 证据：{}，恢复 {recovery_ms} ms",
        project.display()
    );
}

fn prepare_deployment() -> PathBuf {
    let tools = tools();
    let project = tools.project("au11");
    tools.generate(&project);
    tools.plugin(&project, "recovery");
    prepare_plugin(&project);
    configuration(&project, 73);
    command(
        Command::new("cargo")
            .current_dir(&project)
            .args(["build", "--target-dir", "target"]),
    );
    project
}

fn launch(project: &Path) -> Job {
    let tools = tools();
    let binaries = project.join("service/bin");
    fs::create_dir_all(&binaries).unwrap();
    let core = binaries.join("cordis");
    fs::copy(&tools.core, &core).unwrap();
    fs::copy(&tools.host, binaries.join("cordis-host")).unwrap();
    let uid = command(Command::new("id").arg("-u"));
    let uid = String::from_utf8(uid.stdout).unwrap();
    let domain = if uid.trim() == "0" {
        "system".into()
    } else {
        format!("gui/{}", uid.trim())
    };
    let label = format!("com.cordis.acceptance.{}", std::process::id());
    let job = Job {
        target: format!("{domain}/{label}"),
        active: true,
    };
    command(
        Command::new(&core)
            .current_dir(project)
            .args(["service", "install", "--name", &label]),
    );
    job
}

fn restart_from_checkpoint(project: &Path, initial: &JsonValue) -> (JsonValue, u128) {
    until("内存状态检查点", || {
        fs::metadata(project.join("state/recovery.state"))
            .ok()
            .filter(|entry| entry.len() > 0)
    });
    let old_core = initial["pid"].as_u64().unwrap();
    let observation = project.join("state/data/recovery/observation.txt");
    assert_eq!(
        fs::read_to_string(&observation).unwrap(),
        "recovery.value=73 inherited=false"
    );
    let old_hosts = host_pids(initial);
    configuration(project, 29);
    let started = Instant::now();
    command(Command::new("/bin/kill").args(["-KILL", &old_core.to_string()]));
    let restored = until("核心与插件从检查点恢复", || {
        status(project)
            .filter(|snapshot| snapshot["pid"].as_u64() != Some(old_core) && active(snapshot, 1))
    });
    let recovery_ms = started.elapsed().as_millis();
    until("旧插件进程退出", || {
        old_hosts.iter().all(|pid| !exists(*pid)).then_some(())
    });
    let observed = until("继承内存状态的观察结果", || {
        let text = fs::read_to_string(&observation).ok()?;
        text.contains("recovery.value=73 inherited=true")
            .then_some(text)
    });
    assert_eq!(
        observed, "recovery.value=73 inherited=true",
        "重启继承状态覆盖新的初始值"
    );
    (restored, recovery_ms)
}

fn host_pids(snapshot: &JsonValue) -> Vec<u64> {
    snapshot["groups"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|group| group["pid"].as_u64())
        .collect()
}

fn prepare_plugin(project: &Path) {
    let source = project.join("plugins/recovery/src/lib.rs");
    fs::write(source, include_str!("fixtures/recovery.rs")).unwrap();
    let path = project.join("plugins/recovery/Cargo.toml");
    let mut manifest: Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let mut serde = Table::new();
    serde.insert("version".into(), Value::String("=1.0.229".into()));
    serde.insert(
        "features".into(),
        Value::Array(vec![Value::String("derive".into())]),
    );
    manifest["dependencies"]
        .as_table_mut()
        .unwrap()
        .insert("serde".into(), Value::Table(serde));
    fs::write(path, toml::to_string_pretty(&manifest).unwrap()).unwrap();
}

fn configuration(project: &Path, initial: u64) {
    let source = format!(
        "requires_core = \">=0.1.0,<0.2.0\"\n[runtime]\nplugin_dir = \"target/debug\"\ncheckpoint_ms = 100\n[instance.recovery]\nplugin = \"recovery\"\ncomponent = \"recovery\"\nconfig = {{ initial = {initial} }}\n"
    );
    fs::write(project.join("cordis.toml"), source).unwrap();
}

fn exists(pid: u64) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .output()
        .unwrap()
        .status
        .success()
}
