//! DOC-15 RM-7：Rust 使用方计量完整 KV 调用，在内存保留固定数量的原始时延。

use cordis_sdk::prelude::*;
use kv_api::Kv;
use std::{hint::black_box, num::NonZeroUsize, time::Instant};

#[derive(serde::Deserialize)]
struct Config {
    samples: NonZeroUsize,
    key: String,
    value: Vec<u8>,
    label: String,
}

#[derive(serde::Serialize)]
struct Measurement {
    label: String,
    samples: usize,
    median_ns: f64,
    p99_ns: u128,
    wall_ns: u128,
    sample_ns: Vec<u128>,
}

struct Latency;

impl Component for Latency {
    const NAME: &'static str = "latency";
    type Config = Config;

    fn inject() -> Inject {
        Inject::new()
            .with::<dyn Kv>()
            .with::<dyn cordis_sdk::builtin::DataDir>()
    }

    /// RM-7：取得服务与验证请求值后，计量调用、结果复制与释放的完整耗时。
    async fn apply(context: Context, config: Config) -> Result<()> {
        let store = context.get::<dyn Kv>()?;
        store.put(&config.key, config.value.clone())?;
        if store.get(&config.key)? != Some(config.value) {
            return Err(Error::new(2, "测量键需要返回已写入的字节值"));
        }
        let mut samples = Vec::with_capacity(config.samples.get());
        let wall = Instant::now();
        for _ in 0..config.samples.get() {
            let started = Instant::now();
            drop(black_box(store.get(&config.key)?));
            samples.push(started.elapsed().as_nanos());
        }
        let wall_ns = wall.elapsed().as_nanos();
        let mut ordered = samples.clone();
        ordered.sort_unstable();
        let middle = ordered.len() / 2;
        let median = if ordered.len() % 2 == 0 {
            (ordered[middle - 1] as f64 + ordered[middle] as f64) / 2.0
        } else {
            ordered[middle] as f64
        };
        let report = Measurement {
            label: config.label,
            samples: samples.len(),
            median_ns: median,
            p99_ns: ordered[(ordered.len() * 99).div_ceil(100) - 1],
            wall_ns,
            sample_ns: samples,
        };
        let output = context.data_dir()?.join("rpc-latency.json");
        let encoded = serde_json::to_vec_pretty(&report)
            .map_err(|problem| Error::new(2, problem.to_string()))?;
        std::fs::write(output, encoded).map_err(|problem| Error::new(1, problem.to_string()))
    }
}

cordis_sdk::export_plugin!(
    name = "cordis-latency-probe",
    version = "0.1.0",
    components = [Latency]
);
