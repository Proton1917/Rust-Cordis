//! DOC-14 AC-9、DOC-15 AU-10：实际核心子进程中的八线程、五十次替换与有状态阈值重启。

#[path = "acceptance_support/mod.rs"]
pub mod support;

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
use support::{Tools, active, command, status, tools, until, write_json};

struct Artifacts {
    provider: PathBuf,
    version2: PathBuf,
    consumer: PathBuf,
    client: PathBuf,
}

/// Y-3、AC-8：构建两个相同接口版本的真实提供方，使用方由 Rust 1.85 构建。
fn artifacts(tools: &Tools) -> Artifacts {
    let _build = cordis_testkit::build::shared_build_lock(&tools.root).unwrap();
    let build = tools
        .root
        .join("target/stress-artifacts")
        .join(std::process::id().to_string());
    let fixtures = tools
        .root
        .join("crates/cordis-host/tests/fixtures/Cargo.toml");
    let provider = cordis_testkit::build::shared_target_dir(&tools.root);
    let mut build_provider = Command::new("cargo");
    build_provider
        .args(["build", "--release", "--manifest-path"])
        .arg(&fixtures)
        .args(["-p", "loader-fixture", "--target-dir"])
        .arg(&provider);
    command(&mut build_provider);
    fs::create_dir_all(build.join("variants")).unwrap();
    let first = library(&build.join("variants"), "provider-v1");
    fs::copy(library(&provider.join("release"), "loader_fixture"), &first).unwrap();
    build_provider.args(["--features", "implementation2"]);
    command(&mut build_provider);
    let second = library(&build.join("variants"), "provider-v2");
    fs::copy(
        library(&provider.join("release"), "loader_fixture"),
        &second,
    )
    .unwrap();
    command(
        Command::new("cargo")
            .args(["+1.85.0", "build", "--release", "--manifest-path"])
            .arg(&fixtures)
            .args(["-p", "loader-fixture-consumer", "--target-dir"])
            .arg(&provider),
    );
    command(
        Command::new("cargo")
            .args(["build", "--release", "--manifest-path"])
            .arg(
                tools
                    .root
                    .join("crates/cordis-runtime/fixtures/plugin/Cargo.toml"),
            )
            .arg("--target-dir")
            .arg(&provider),
    );
    let consumer_name = format!(
        "{}loader_fixture_consumer{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let copied = cordis_testkit::build::snapshot_artifacts(
        &tools.root,
        "stress-fixtures",
        "release",
        &[&consumer_name, "cordis-runtime-fixture"],
    )
    .unwrap();
    Artifacts {
        provider: first,
        version2: second,
        consumer: library(&copied, "loader_fixture_consumer"),
        client: copied.join("cordis-runtime-fixture"),
    }
}

fn library(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!(
        "{}{}{}",
        std::env::consts::DLL_PREFIX,
        name,
        std::env::consts::DLL_SUFFIX
    ))
}

fn quote(path: &Path) -> String {
    toml::Value::String(path.to_string_lossy().into_owned()).to_string()
}

/// AC-9：管理者保持独立，线程使用方和 idle 与提供方共享原生宿主。
fn deployment(tools: &Tools, artifacts: &Artifacts) -> PathBuf {
    let project = tools.project("ac9");
    fs::create_dir_all(project.join("plugins")).unwrap();
    fs::create_dir_all(project.join("sources")).unwrap();
    fs::create_dir_all(project.join("state/data/loader-consumer")).unwrap();
    let provider = library(&project.join("plugins"), "provider");
    let consumer = library(&project.join("plugins"), "consumer");
    fs::copy(&artifacts.provider, &provider).unwrap();
    fs::copy(&artifacts.consumer, &consumer).unwrap();
    for (role, directory) in [("admin", "admin"), ("native-client", "client")] {
        let directory = project.join("plugins").join(directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("cordis-plugin.toml"), format!(
            "name = 'fixture-{role}'\nversion = '0.1.0'\nsocket = 'process'\nprotocol = 1\ncommand = [{}, '{role}', 'v1']\n",
            quote(&artifacts.client))).unwrap();
    }
    let configuration = format!(
        "[runtime]\nconfirm_ms = 0\ncheckpoint_ms = 0\nretired_image_limit = 20\n\n[instance.store]\nplugin = {}\ncomponent = 'provider'\ngroup = 'native'\nconfig = {{ initial = 100 }}\n\n[instance.callers]\nplugin = {}\ncomponent = 'callers'\ngroup = 'native'\nconfig = {{ observation = {} }}\n\n[instance.idle]\nplugin = {}\ncomponent = 'idle'\ngroup = 'native'\n\n[instance.admin]\nplugin = 'admin'\ngrants = {{ 'cordis.manage@1' = ['*'] }}\nconfig = {{ commands = {}, results = {} }}\n\n[instance.client]\nplugin = 'client'\nconfig = {{ commands = {}, results = {} }}\n",
        quote(Path::new(provider.file_name().unwrap())),
        quote(Path::new(consumer.file_name().unwrap())),
        quote(&project.join("state/data/loader-consumer/callers.cbor")),
        quote(Path::new(consumer.file_name().unwrap())),
        quote(&project.join("commands")),
        quote(&project.join("results")),
        quote(&project.join("client-commands")),
        quote(&project.join("client-results"))
    );
    fs::write(project.join("cordis.toml"), configuration).unwrap();
    project
}

/// AC-9：文件命令驱动真实进程插件，rpc_wall_us 由该插件在 get/invoke 周围计时。
fn request(project: &Path, id: u64, interface: &str, method: &str, args: Value) -> Value {
    let prefix = if interface.starts_with("loader.counter@") {
        "client-"
    } else {
        ""
    };
    let directory = project.join(format!("{prefix}commands"));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{id:04}.json"));
    let staged = path.with_extension("next");
    write_json(
        &staged,
        &json!({"interface":interface,"method":method,"args":args}),
    );
    fs::rename(staged, path).unwrap();
    let reply = until("真实管理调用返回", || {
        fs::read(project.join(format!("{prefix}results/{id:04}.json")))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    });
    assert_eq!(reply["ok"], true, "{reply}");
    reply
}

fn instance<'a>(snapshot: &'a Value, name: &str) -> &'a Value {
    snapshot["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .unwrap()
}

fn native_pid(snapshot: &Value) -> u64 {
    snapshot["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["name"] == "native")
        .unwrap()["pid"]
        .as_u64()
        .unwrap()
}

/// AC-9：每个加载纪元先清空观察文件，八个实际线程的首个读取结果共同形成确认点。
fn observed(project: &Path, pid: u64, version: u64, value: u64) -> Value {
    until("八个线程观察本轮状态和实现", || {
        let bytes = fs::read(project.join("state/data/loader-consumer/callers.cbor")).ok()?;
        let record: Value = cordis_up::decode_value(&bytes, &cordis_up::Limits::DEFAULT)
            .ok()?
            .deserialized()
            .ok()?;
        (record["pid"] == pid
            && record["complete"] == true
            && record["versions"].as_array()?.len() == 8
            && record["versions"]
                .as_array()?
                .iter()
                .all(|item| item == version)
            && record["values"]
                .as_array()?
                .iter()
                .all(|item| item == value))
        .then_some(record)
    })
}

/// RM-6：按当前核心和分组PID读取 ps 的 KiB RSS，逐PID记录并汇总当前进程集合。
fn memory(snapshot: &Value) -> Value {
    let core = snapshot["pid"].as_u64().unwrap();
    let mut pids = vec![core];
    pids.extend(
        snapshot["groups"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|group| group["pid"].as_u64()),
    );
    let output = command(
        Command::new("ps").args(["-o", "pid=,rss=", "-p"]).arg(
            pids.iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    let records = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            (
                fields.next().unwrap().parse::<u64>().unwrap(),
                fields.next().unwrap().parse::<u64>().unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(records.len(), pids.len(), "RSS覆盖实际运行的核心与全部分组");
    json!({"core_kib":records[&core],"native_kib":records[&native_pid(snapshot)],
        "total_kib":records.values().sum::<u64>(),"processes":records})
}

/// RM-3/RM-4：依据实际公开报告等待最终统计，21/42轮包含整组重启带来的 idle。
fn round_snapshot(project: &Path, reference: &Value, changed: usize, previous: &Value) -> Value {
    let previous_candidate = previous["last_swap"]
        .as_array()
        .unwrap()
        .iter()
        .find(|report| report["plugin"] == "loader-fixture")
        .map(|report| &report["candidate_library"]);
    until("本轮完整替换报告发布", || {
        let snapshot = status(project)?;
        let report = snapshot["last_swap"]
            .as_array()?
            .iter()
            .find(|report| report["plugin"] == "loader-fixture")?;
        (active(&snapshot, 5)
            && instance(&snapshot, "store")["reference"] == *reference
            && Some(&report["candidate_library"]) != previous_candidate
            && report["succeeded"] == true
            && report["affected_instances"] == changed)
            .then_some(snapshot)
    })
}

fn distribution(samples: &[Value], field: &str) -> Value {
    let mut values = samples
        .iter()
        .map(|sample| sample[field].as_f64().unwrap())
        .collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    json!({"median":(values[24]+values[25])/2.0,"max":values[49]})
}

/// AC-9、AU-10：固定50轮，成功后只汇总既定原始观测。
#[test]
fn eight_threads_survive_fifty_native_replacements_with_stateful_restarts() {
    let tools = tools();
    let artifacts = artifacts(tools);
    let project = deployment(tools, &artifacts);
    let mut running = tools.start(&project);
    let initial = until("压力部署全部Active", || {
        status(&project).filter(|snapshot| active(snapshot, 5))
    });
    eprintln!(
        "AC-9开始：test_pid={} core_pid={} native_pid={} project={} 固定50轮、8线程",
        std::process::id(),
        running.0.id(),
        native_pid(&initial),
        project.display()
    );
    observed(&project, native_pid(&initial), 1, 100);
    assert_eq!(
        request(&project, 1, "loader.counter@1.0", "add", json!([37]))["value"],
        137
    );
    let references = install_versions(&project, &artifacts);
    let initial_memory = memory(&initial);
    let samples = replace_rounds(&project, &references, initial, initial_memory.clone());
    running.stop();
    let final_memory = &samples.last().unwrap()["rss"];
    write_json(
        &project.join("result.json"),
        &json!({
            "scenario":"AC-9/AU-10", "threads":8, "replacements":50,"state":137,"restarts":[21,42],
            "profile":"release","compiler_consumer":"1.85.0","rss_unit":"KiB", "initial_rss":initial_memory,
            "RM-3":distribution(&samples,"affected_instances"),"RM-4_ms":distribution(&samples,"pause_ms"),
            "wall_ms":distribution(&samples,"wall_ms"),"harness_wall_ms":distribution(&samples,"harness_wall_ms"),
            "RM-6_total_kib_per_swap":(final_memory["total_kib"].as_i64().unwrap()-initial_memory["total_kib"].as_i64().unwrap()) as f64/50.0,
            "RM-6_core_kib_per_swap":(final_memory["core_kib"].as_i64().unwrap()-initial_memory["core_kib"].as_i64().unwrap()) as f64/50.0,
            "samples":samples
        }),
    );
    eprintln!("AC-9/RM-3/RM-4/RM-6证据：{}", project.display());
}

fn install_versions(project: &Path, artifacts: &Artifacts) -> [Value; 2] {
    let mut references = Vec::new();
    for (index, source) in [&artifacts.provider, &artifacts.version2]
        .into_iter()
        .enumerate()
    {
        let path = library(&project.join("sources"), &format!("provider-{}", index + 1));
        fs::copy(source, &path).unwrap();
        references.push(
            request(
                project,
                10 + index as u64,
                "cordis.manage@1.1",
                "install",
                json!([path]),
            )["value"]
                .clone(),
        );
    }
    references.try_into().unwrap()
}

/// AC-9：每轮验证实现、状态、八线程观测及 idle 纪元，实际重启时核对常驻内存回落。
fn replace_rounds(
    project: &Path,
    references: &[Value; 2],
    mut previous: Value,
    mut previous_memory: Value,
) -> Vec<Value> {
    let mut samples = Vec::new();
    for round in 1..=50_u64 {
        let version = if round % 2 == 1 { 2 } else { 1 };
        let reference = &references[(version - 1) as usize];
        let restarted = matches!(round, 21 | 42);
        let started = Instant::now();
        let reply = request(
            project,
            100 + round,
            "cordis.manage@1.1",
            "replace",
            json!(["store", reference]),
        );
        let harness_wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        current_counter(project, round, version);
        let snapshot = round_snapshot(project, reference, if restarted { 4 } else { 3 }, &previous);
        let observation = observed(project, native_pid(&snapshot), version, 137);
        let rss = restart_observation(&previous, &snapshot, &previous_memory, restarted, round);
        let report = &snapshot["last_swap"]
            .as_array()
            .unwrap()
            .iter()
            .find(|report| report["plugin"] == "loader-fixture")
            .unwrap();
        let wall_ms = reply["rpc_wall_us"].as_u64().unwrap() as f64 / 1000.0;
        assert!(
            wall_ms >= report["pause_ms"].as_u64().unwrap() as f64,
            "完整RPC调用覆盖RM-4"
        );
        samples.push(json!({"round":round,"implementation":version,"value":137,"restarted":restarted,
            "affected_instances":report["affected_instances"],"pause_ms":report["pause_ms"],"rpc_wall_us":reply["rpc_wall_us"],
            "wall_ms":wall_ms,"harness_wall_ms":harness_wall_ms,"quiescence_ms":report["quiescence_ms"],
            "native_pid":native_pid(&snapshot),"idle_revision":instance(&snapshot,"idle")["revision"],"threads":observation,"rss":rss}));
        previous = snapshot;
        previous_memory = rss;
    }
    samples
}

/// AC-9：在管理调用返回后，经独立进程客户端读取真实状态与实现编号。
fn current_counter(project: &Path, round: u64, version: u64) {
    assert_eq!(
        request(
            project,
            200 + round,
            "loader.counter@1.0",
            "value",
            json!([])
        )["value"],
        137
    );
    assert_eq!(
        request(
            project,
            300 + round,
            "loader.counter@1.0",
            "implementation",
            json!([])
        )["value"],
        version
    );
}

/// AU-10、RM-6：同一采样位置比较宿主身份、idle纪元与原生进程的实际RSS。
fn restart_observation(
    previous: &Value,
    snapshot: &Value,
    previous_memory: &Value,
    restarted: bool,
    round: u64,
) -> Value {
    assert_eq!(
        native_pid(snapshot) != native_pid(previous),
        restarted,
        "第{round}轮宿主重启条件"
    );
    assert_eq!(
        instance(snapshot, "idle")["revision"] != instance(previous, "idle")["revision"],
        restarted,
        "第{round}轮idle纪元"
    );
    let rss = memory(snapshot);
    if restarted {
        assert!(
            rss["native_kib"].as_u64().unwrap() < previous_memory["native_kib"].as_u64().unwrap(),
            "第{round}轮RSS应回落：{previous_memory} -> {rss}"
        );
    }
    rss
}
