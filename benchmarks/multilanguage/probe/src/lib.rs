//! DOC-15 RM-7：Rust 使用方计量完整 KV 调用，在内存保留固定数量的原始时延。

use cordis_sdk::prelude::*;
use kv_api::Kv;
use std::{
    hint::black_box,
    num::NonZeroUsize,
    sync::{Condvar, Mutex, mpsc},
    time::Instant,
};

#[derive(serde::Deserialize)]
struct Config {
    samples: NonZeroUsize,
    #[serde(default)]
    warmup: usize,
    #[serde(default = "one")]
    concurrency: NonZeroUsize,
    key: String,
    #[serde(default)]
    value: Option<Vec<u8>>,
    #[serde(default)]
    bytes: Option<usize>,
    label: String,
}

#[derive(serde::Serialize)]
struct Measurement {
    label: String,
    samples: usize,
    warmup: usize,
    concurrency: usize,
    payload_bytes: usize,
    median_ns: f64,
    p99_ns: u128,
    wall_ns: u128,
    sample_ns: Vec<u128>,
}

struct Latency;

fn one() -> NonZeroUsize {
    NonZeroUsize::new(1).unwrap()
}

/// 各工作线程就绪后同时放行，作用域在成功、错误及 panic 时均回收线程。
fn concurrent_reads(
    store: &Service<dyn Kv>,
    key: &str,
    value: &[u8],
    count: usize,
    concurrency: usize,
) -> Result<(Vec<u128>, u128)> {
    let gate = (Mutex::new(None::<bool>), Condvar::new());
    std::thread::scope(|scope| {
        let (ready, observed) = mpsc::channel();
        let mut workers = Vec::with_capacity(concurrency);
        for index in 0..concurrency {
            let calls = count / concurrency + usize::from(index < count % concurrency);
            let ready = ready.clone();
            let gate = &gate;
            let worker = std::thread::Builder::new().spawn_scoped(scope, move || {
                let mut samples = Vec::with_capacity(calls);
                let _receiver = ready.send(());
                drop(ready);
                let (lock, condition) = gate;
                let mut start = lock.lock().unwrap();
                while start.is_none() {
                    start = condition.wait(start).unwrap();
                }
                if *start == Some(false) {
                    return Ok((samples, Instant::now()));
                }
                drop(start);
                for _ in 0..calls {
                    let started = Instant::now();
                    let received = store.get(key)?;
                    if received.as_deref() != Some(value) {
                        return Err(Error::new(2, "并发测量返回值与写入字节有差异"));
                    }
                    drop(black_box(received));
                    samples.push(started.elapsed().as_nanos());
                }
                Ok((samples, Instant::now()))
            });
            match worker {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    *gate.0.lock().unwrap() = Some(false);
                    gate.1.notify_all();
                    return Err(Error::new(1, error.to_string()));
                }
            }
        }
        drop(ready);
        if observed.into_iter().count() != concurrency {
            *gate.0.lock().unwrap() = Some(false);
            gate.1.notify_all();
            return Err(Error::new(5, "并发测量工作线程准备失败"));
        }
        let wall = Instant::now();
        *gate.0.lock().unwrap() = Some(true);
        gate.1.notify_all();
        let mut samples = Vec::with_capacity(count);
        let mut finished = wall;
        let mut failure = None;
        for worker in workers {
            match worker.join() {
                Ok(Ok((mut values, ended))) => {
                    samples.append(&mut values);
                    finished = finished.max(ended);
                }
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert_with(|| Error::new(5, "并发测量工作线程发生 panic"));
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok((samples, finished.duration_since(wall).as_nanos()))
    })
}

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
        let concurrency = config.concurrency.get();
        if concurrency > config.samples.get() {
            return Err(Error::new(2, "并发数需要小于等于测量样本数"));
        }
        let value = match (config.value, config.bytes) {
            (Some(value), None) => value,
            (None, Some(bytes)) => (0..bytes).map(|i| (i % 251) as u8).collect(),
            _ => return Err(Error::new(2, "测量配置需要单独指定 value 或 bytes")),
        };
        let payload_bytes = value.len();
        let store = context.get::<dyn Kv>()?;
        store.put(&config.key, value.clone())?;
        if store.get(&config.key)?.as_deref() != Some(value.as_slice()) {
            return Err(Error::new(2, "测量键需要返回已写入的字节值"));
        }
        let expected = (concurrency > 1).then_some(value);
        for _ in 0..config.warmup {
            drop(black_box(store.get(&config.key)?));
        }
        let (samples, wall_ns) = if let Some(expected) = expected {
            // 等待作用域线程时交还原生执行线程，单原生工作线程也能推进 RPC。
            context.wait(async {
                concurrent_reads(
                    &store,
                    &config.key,
                    &expected,
                    config.samples.get(),
                    concurrency,
                )
            })?
        } else {
            let mut samples = Vec::with_capacity(config.samples.get());
            let wall = Instant::now();
            for _ in 0..config.samples.get() {
                let started = Instant::now();
                drop(black_box(store.get(&config.key)?));
                samples.push(started.elapsed().as_nanos());
            }
            (samples, wall.elapsed().as_nanos())
        };
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
            warmup: config.warmup,
            concurrency,
            payload_bytes,
            median_ns: median,
            p99_ns: ordered[(ordered.len() * 99).div_ceil(100) - 1],
            wall_ns,
            sample_ns: samples,
        };
        publish(context, "rpc-latency.json", &report)
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum StateAction {
    Seed,
    Restore,
}

#[derive(serde::Deserialize)]
struct StateConfig {
    action: StateAction,
    bytes: usize,
}

struct StateProbe;

impl Component for StateProbe {
    const NAME: &'static str = "state-probe";
    type Config = StateConfig;

    fn inject() -> Inject {
        Latency::inject()
    }

    async fn apply(context: Context, config: StateConfig) -> Result<()> {
        let store = context.get::<dyn Kv>()?;
        let key = "checkpoint-payload";
        if matches!(config.action, StateAction::Seed) {
            store.put(key, (0..config.bytes).map(|i| (i % 251) as u8).collect())?;
        }
        let value = store
            .get(key)?
            .ok_or_else(|| Error::new(4, "KV 缺少检查点中的业务值"))?;
        if value.len() != config.bytes
            || value
                .iter()
                .enumerate()
                .any(|(i, &byte)| byte != (i % 251) as u8)
        {
            return Err(Error::new(2, "KV 继承值与初始写入的字节序列有差异"));
        }
        publish(context, "inherited.json", &value.len())
    }
}

fn publish(context: Context, name: &str, result: &impl serde::Serialize) -> Result<()> {
    let path = context.data_dir()?.join(name);
    let staged = path.with_extension("next");
    let bytes = serde_json::to_vec(result).map_err(|e| Error::new(2, e.to_string()))?;
    std::fs::write(&staged, bytes)
        .and_then(|()| std::fs::rename(staged, path))
        .map_err(|e| Error::new(1, e.to_string()))
}

cordis_sdk::export_plugin!(
    name = "cordis-latency-probe",
    version = "0.1.0",
    components = [Latency, StateProbe]
);
