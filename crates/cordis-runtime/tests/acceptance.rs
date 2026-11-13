//! DOC-14 AC-1、RM-2 与 DOC-15 AU-11：运行核心的装载和服务管理器恢复验收。

#[path = "acceptance_support/mod.rs"]
mod support;

use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, process::Command, time::Instant};
use support::{active, command, status, tools, until, write_json};
use toml::Table;

#[test]
fn eight_independent_libraries_and_one_hundred_instances() {
    let tools = tools();
    let project = tools.project("ac1");
    tools.generate(&project);
    for name in [
        "load-one",
        "load-two",
        "load-three",
        "load-four",
        "load-five",
        "load-six",
    ] {
        tools.plugin(&project, name);
    }
    populate_instances(&project);
    let built = command(Command::new("cargo").current_dir(&project).args([
        "build",
        "--target-dir",
        "target",
        "--message-format=json",
    ]));
    fs::write(project.join("build.jsonl"), built.stdout).unwrap();
    fs::write(project.join("build.stderr.log"), built.stderr).unwrap();
    let started = Instant::now();
    let mut running = tools.start(&project);
    let snapshot = until("100 个实例进入 active", || {
        assert!(
            running.0.try_wait().unwrap().is_none(),
            "核心退出，证据 {}",
            project.display()
        );
        status(&project).filter(|snapshot| active(snapshot, 100))
    });
    let startup_ms = started.elapsed().as_millis();
    let names: BTreeSet<_> = snapshot["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|library| library["retired"] == false)
        .map(|library| library["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names.len(), 8, "运行状态列出八个独立编译的库");
    write_json(&project.join("active-status.json"), &snapshot);
    running.stop();
    let rebuilt = rebuild_one(&project);
    write_json(
        &project.join("result.json"),
        &json!({"scenario":"AC-1", "libraries":names, "instances":100, "startup_ms":startup_ms,"RM-2":rebuilt}),
    );
    fs::remove_dir_all(project.join("target")).unwrap();
    eprintln!(
        "AC-1 / RM-2 证据：{}，启动 {startup_ms} ms",
        project.display()
    );
}

fn populate_instances(project: &std::path::Path) {
    let config_path = project.join("cordis.toml");
    let mut config: Table = toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    let instances = config["instance"].as_table_mut().unwrap();
    for (name, definition) in instances.clone() {
        let repetitions = match name.as_str() {
            "watch" => 1,
            "app" => 15,
            _ => 14,
        };
        for index in 1..repetitions {
            instances.insert(format!("{name}-{index}"), definition.clone());
        }
    }
    fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
}

/// RM-2：改变一个插件的激活消息，编译记录应只出现该工作区成员的一次实际编译。
fn rebuild_one(project: &std::path::Path) -> Vec<String> {
    let source = project.join("plugins/load-one/src/lib.rs");
    let original = fs::read_to_string(&source).unwrap();
    fs::write(source, original.replace("已激活", "已更新")).unwrap();
    let output = command(Command::new("cargo").current_dir(project).args([
        "build",
        "--target-dir",
        "target",
        "--message-format=json",
    ]));
    fs::write(project.join("rebuild.jsonl"), &output.stdout).unwrap();
    fs::write(project.join("rebuild.stderr.log"), &output.stderr).unwrap();
    let changed: Vec<_> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|message| message["reason"] == "compiler-artifact" && message["fresh"] == false)
        .map(|message| message["target"]["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(changed, ["load_one"], "仅修改的插件重新编译");
    changed
}

#[test]
fn readme_first_plugin_builds_from_pinned_git_sdk() {
    let tools = tools();
    let project = tools.project("readme");
    std::fs::create_dir(&project).unwrap();
    command(
        Command::new(&tools.core)
            .env("PATH", tools.path())
            .arg("new")
            .arg(&project),
    );
    tools.plugin(&project, "hello");
    let manifest: Table =
        toml::from_str(&fs::read_to_string(project.join("Cargo.toml")).unwrap()).unwrap();
    let sdk = &manifest["workspace"]["dependencies"]["cordis-sdk"];
    let revision = sdk["rev"].as_str().unwrap();
    assert_eq!(
        sdk["git"].as_str(),
        Some("https://github.com/Proton1917/Rust-Cordis.git")
    );
    let built = command(Command::new("cargo").current_dir(&project).args([
        "build",
        "--target-dir",
        "target",
    ]));
    fs::write(project.join("build.stderr.log"), &built.stderr).unwrap();
    let mut running = tools.start(&project);
    let observed = until("README 首个插件 active", || {
        assert!(
            running.0.try_wait().unwrap().is_none(),
            "核心退出，证据 {}",
            project.display()
        );
        status(&project).filter(|snapshot| active(snapshot, 3))
    });
    assert!(
        observed["instances"]
            .as_array()
            .unwrap()
            .iter()
            .any(|instance| instance["name"] == "hello" && instance["state"] == "active")
    );
    running.stop();
    write_json(
        &project.join("result.json"),
        &json!({"scenario":"README first plugin", "sdk_revision":revision, "instances":["app","watch","hello"]}),
    );
    fs::remove_dir_all(project.join("target")).unwrap();
    eprintln!("README 首个插件证据：{}", project.display());
}

#[cfg(target_os = "macos")]
#[test]
fn launchd_restores_core_and_checkpoint_after_core_exit() {
    service::recover();
}

#[cfg(target_os = "macos")]
#[path = "acceptance_service/mod.rs"]
mod service;
