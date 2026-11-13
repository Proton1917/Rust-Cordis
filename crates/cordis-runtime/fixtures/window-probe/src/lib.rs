//! AU-17：真实原生使用方记录跨进程开关窗口及当前激活期间的 closed 事件。

use cordis_sdk::{builtin::DataDir, prelude::*};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
use window_api::{Closed, Window};

static NEXT_ACTIVATION: AtomicU64 = AtomicU64::new(1);

#[derive(serde::Deserialize)]
pub struct Config {
    pub url: String,
}

#[derive(serde::Serialize)]
struct Observation {
    activation: u64,
    window: u64,
}

/// AU-17：观察记录绑定真实 apply 的身份和窗口编号，编码与读端共用 CBOR。
fn record(path: &Path, activation: u64, window: u64) -> Result<()> {
    let bytes = cordis_sdk::encode(&Observation { activation, window })?;
    fs::write(path, bytes).map_err(|problem| Error::new(1, problem.to_string()))
}

pub struct Probe;

impl Component for Probe {
    const NAME: &'static str = "window-probe";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new().with::<dyn Window>().with::<dyn DataDir>()
    }
    fn listens() -> Listens {
        Listens::new().with::<Closed>()
    }

    /// AU-17：每次 apply 分配观察身份，open 返回后 close，事件回调捕获同一身份。
    async fn apply(context: Context, config: Config) -> Result<()> {
        let activation = NEXT_ACTIVATION.fetch_add(1, Ordering::Relaxed);
        let directory = context.data_dir()?;
        let closed = directory.join("closed.cbor");
        let called = directory.join("called.cbor");
        context.on_async::<Closed, _, _>(move |event| {
            let closed = closed.clone();
            async move { record(&closed, activation, event.id) }
        })?;
        let window = context.get::<dyn Window>()?;
        let id = window
            .open(&config.url, "Cordis 窗口调用", 640, 420)
            .await?;
        window.close(id).await?;
        record(&called, activation, id)
    }
}

cordis_sdk::export_plugin!(
    name = "window-probe",
    version = "0.1.0",
    components = [Probe]
);
