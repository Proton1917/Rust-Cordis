//! DOC-15 AU-9、RM-9：固定轮次交替测量 M9 与当前 SDK 的同组类型化调用。

use super::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub(super) struct Settings {
    pub baseline: PathBuf,
    pub current: PathBuf,
    pub rounds: u32,
    pub warmup: u64,
    pub iterations: u64,
    pub median_ratio_limit: f64,
    pub maximum_ratio_limit: f64,
}

#[derive(Deserialize, Serialize)]
struct Sample {
    implementation: String,
    round: u32,
    iterations: u64,
    elapsed_ns: u128,
    last_value: u64,
}

#[derive(Serialize)]
struct Measurement {
    baseline_commit: &'static str,
    baseline_library: PathBuf,
    current_library: PathBuf,
    builds: Vec<Build>,
    profile: &'static str,
    warmup_per_round: u64,
    iterations_per_round: u64,
    expected_rounds_per_implementation: u32,
    baseline_median_ns: Option<f64>,
    current_median_ns: Option<f64>,
    median_ratio: Option<f64>,
    maximum_ratio: Option<f64>,
    paired_ratios: Vec<f64>,
    median_ratio_limit: f64,
    maximum_ratio_limit: f64,
    failure: Option<String>,
    samples: Vec<Sample>,
}

#[derive(Serialize)]
struct Build {
    group: String,
    plugin: String,
    information: String,
}

pub(super) async fn run(artifacts: &Artifacts, settings: &Settings) -> Result<()> {
    check(
        settings.rounds != 0 && settings.iterations != 0,
        "时延测量需要正数轮次与调用次数",
    )?;
    let context = Context::new(Arc::new(Executor(Instant::now())));
    let manager = Manager::new(
        context.clone(),
        ManagerOptions {
            apply_timeout: Duration::from_secs(5),
            cleanup_timeout: Duration::from_secs(5),
            ..ManagerOptions::default()
        },
    )
    .await?;
    let logs = Arc::new(Mutex::new(Vec::new()));
    manager.set_log_sink({
        let logs = logs.clone();
        Arc::new(move |record| logs.lock().unwrap().push(record))
    });
    let mut samples = Vec::new();
    let mut builds = Vec::new();
    let outcome = execute(
        &manager,
        artifacts,
        settings,
        &logs,
        &mut samples,
        &mut builds,
    )
    .await;
    let cleanup = manager.shutdown().await;
    let disposed = context.fiber().dispose().await;
    let mut report = measurement(settings, samples, builds);
    let outcome = outcome
        .and_then(|()| cleanup.map_err(Into::into))
        .and_then(|()| disposed.map_err(Into::into))
        .and_then(|()| assess(&report));
    report.failure = outcome.as_ref().err().map(ToString::to_string);
    let file = artifacts.work.join("direct-call-measurements.json");
    serde_json::to_writer_pretty(std::fs::File::create(&file)?, &report)?;
    eprintln!("直接调用测量已写入{}", file.display());
    outcome
}

async fn execute(
    manager: &Manager,
    artifacts: &Artifacts,
    settings: &Settings,
    logs: &Mutex<Vec<cordis_supervisor::manager::LogRecord>>,
    samples: &mut Vec<Sample>,
    builds: &mut Vec<Build>,
) -> Result<()> {
    let mut views = Vec::new();
    for (label, consumer) in [
        ("baseline", &settings.baseline),
        ("current", &settings.current),
    ] {
        let group = manager
            .attach_group(group(
                label,
                &artifacts.work.join(label),
                artifacts,
                &[&artifacts.provider, consumer],
            )?)
            .await?;
        for plugin in group.plugins {
            check(
                plugin.build_info.contains("1.85.0"),
                "计时consumer与provider统一使用Rust1.85.0",
            )?;
            builds.push(Build {
                group: label.into(),
                plugin: plugin.name,
                information: plugin.build_info,
            });
        }
        let view = manager
            .context()
            .isolate_named("loader.counter@1", cordis::Realm::new());
        let provider = manager
            .mount_in(
                view.clone(),
                InstanceSpec {
                    name: format!("{label}-provider"),
                    group: label.into(),
                    plugin: "loader-fixture".into(),
                    component: "provider".into(),
                    config: Value::Map(Vec::new()),
                },
                None,
            )
            .await?;
        provider.settled().await?;
        views.push(view);
    }
    for round in 0..settings.rounds {
        let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
        for index in order {
            let label = ["baseline", "current"][index];
            samples
                .push(one_round(manager, &views[index], logs, settings, label, round + 1).await?);
        }
    }
    for label in ["baseline", "current"] {
        manager.dispose(&format!("{label}-provider")).await?;
    }
    Ok(())
}

async fn one_round(
    manager: &Manager,
    view: &Context,
    logs: &Mutex<Vec<cordis_supervisor::manager::LogRecord>>,
    settings: &Settings,
    label: &str,
    round: u32,
) -> Result<Sample> {
    let name = format!("{label}-{round}");
    let config = Value::Map(vec![
        ("warmup".into(), settings.warmup.into()),
        ("iterations".into(), settings.iterations.into()),
        ("round".into(), round.into()),
        ("implementation".into(), label.into()),
    ]);
    let instance = manager
        .mount_in(
            view.clone(),
            InstanceSpec {
                name: name.clone(),
                group: label.into(),
                plugin: "direct-benchmark".into(),
                component: "benchmark".into(),
                config,
            },
            None,
        )
        .await?;
    instance.settled().await?;
    let measurement: Sample = {
        let logs = logs.lock().unwrap();
        let record = logs
            .iter()
            .find(|record| record.instance == name)
            .ok_or_else(|| std::io::Error::other("原生计时结果应经实际日志请求到达核心"))?;
        serde_json::from_str(&record.message)?
    };
    check(
        measurement.last_value == 100,
        "被测直接调用应返回提供方实际初值100",
    )?;
    manager.dispose(&name).await?;
    eprintln!(
        "{label} round {round}: {:.3} ns/call",
        measurement.elapsed_ns as f64 / measurement.iterations as f64
    );
    Ok(measurement)
}

fn measurement(settings: &Settings, samples: Vec<Sample>, builds: Vec<Build>) -> Measurement {
    let values = |label| {
        samples
            .iter()
            .filter(|sample| sample.implementation == label)
            .map(|sample| sample.elapsed_ns as f64 / sample.iterations as f64)
            .collect::<Vec<_>>()
    };
    let baseline = values("baseline");
    let current = values("current");
    let paired_ratios = current
        .iter()
        .zip(&baseline)
        .map(|(current, baseline)| current / baseline)
        .collect();
    let baseline_median_ns = median(baseline.clone());
    let current_median_ns = median(current.clone());
    let baseline_max = baseline.iter().copied().reduce(f64::max);
    let current_max = current.iter().copied().reduce(f64::max);
    Measurement {
        baseline_commit: "dc93da1",
        baseline_library: settings.baseline.clone(),
        current_library: settings.current.clone(),
        builds,
        profile: "release opt-level=3",
        warmup_per_round: settings.warmup,
        iterations_per_round: settings.iterations,
        expected_rounds_per_implementation: settings.rounds,
        baseline_median_ns,
        current_median_ns,
        median_ratio: current_median_ns
            .zip(baseline_median_ns)
            .map(|(current, baseline)| current / baseline),
        maximum_ratio: current_max
            .zip(baseline_max)
            .map(|(current, baseline)| current / baseline),
        paired_ratios,
        median_ratio_limit: settings.median_ratio_limit,
        maximum_ratio_limit: settings.maximum_ratio_limit,
        failure: None,
        samples,
    }
}

fn assess(measurement: &Measurement) -> Result<()> {
    check(
        measurement.samples.len() == measurement.expected_rounds_per_implementation as usize * 2,
        "基准与当前实现应完成全部固定轮次",
    )?;
    check(
        measurement
            .median_ratio
            .is_some_and(|ratio| ratio <= measurement.median_ratio_limit),
        format!(
            "中位数比值{:?}应满足预定上限{}",
            measurement.median_ratio, measurement.median_ratio_limit
        ),
    )?;
    check(
        measurement
            .maximum_ratio
            .is_some_and(|ratio| ratio <= measurement.maximum_ratio_limit),
        format!(
            "最大值比值{:?}应满足预定上限{}",
            measurement.maximum_ratio, measurement.maximum_ratio_limit
        ),
    )
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}
