use cordis_sdk::prelude::*;
use std::time::Instant;

#[cordis_sdk::interface(name = "loader.counter", version = "1.0")]
trait Counter: Send + Sync {
    fn value(&self) -> Result<u64>;
    fn add(&self, amount: u64) -> Result<u64>;
    fn implementation(&self) -> Result<u64>;
}

#[derive(serde::Deserialize)]
struct Configuration {
    warmup: u64,
    iterations: u64,
    round: u32,
    implementation: String,
}

#[derive(serde::Serialize)]
struct Measurement {
    implementation: String,
    round: u32,
    iterations: u64,
    elapsed_ns: u128,
    last_value: u64,
}

struct Benchmark;
impl Component for Benchmark {
    const NAME: &'static str = "benchmark";
    type Config = Configuration;

    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>()
    }

    async fn apply(ctx: Context, config: Configuration) -> Result<()> {
        let service = ctx.get::<dyn Counter>()?;
        if !service.clone_object()?.has_typed_methods() {
            return Err(Error::new(1010, "计时对象应采用类型化方法表"));
        }
        for _ in 0..config.warmup {
            std::hint::black_box(service.value()?);
        }
        let started = Instant::now();
        let mut last_value = 0;
        for _ in 0..config.iterations {
            last_value = std::hint::black_box(service.value()?);
        }
        let elapsed_ns = started.elapsed().as_nanos();
        let measurement = Measurement {
            implementation: config.implementation,
            round: config.round,
            iterations: config.iterations,
            elapsed_ns,
            last_value,
        };
        let message = serde_json::to_string(&measurement)
            .map_err(|error| Error::new(2, error.to_string()))?;
        ctx.log(2, &message)
    }
}

cordis_sdk::export_plugin!(
    name = "direct-benchmark",
    version = env!("CARGO_PKG_VERSION"),
    components = [Benchmark]
);
