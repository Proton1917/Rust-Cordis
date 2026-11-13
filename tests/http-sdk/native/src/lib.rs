//! ADR-0046：真实原生使用方验证聚合字节载荷与通用流。
#![forbid(unsafe_code)]

mod bytes_binding;
mod conflicts;
mod identity;
mod secrets;
mod streams;
mod threads;
use bytes_binding::{Bundle, Exchange, Part, Selection};
use cordis_sdk::prelude::*;

struct Echo;
#[cordis_sdk::interface]
impl Exchange for Echo {
    async fn exchange(&self, value: Bundle) -> Result<Bundle> {
        Ok(value)
    }
}

struct Provider;
impl Component for Provider {
    const NAME: &'static str = "bytes-provider";
    type Config = EmptyConfig;
    fn provides() -> Provides {
        Provides::new().with::<dyn Exchange>()
    }
    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        context.provide::<dyn Exchange>(Arc::new(Echo))
    }
}

#[derive(serde::Deserialize)]
struct ProbeConfig {
    result: String,
}

struct Probe;
impl Component for Probe {
    const NAME: &'static str = "bytes-probe";
    type Config = ProbeConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn Exchange>()
    }
    /// ADR-0039 M22：五种聚合各持有独立 1 MiB 数据，经跨组服务实际返回后逐字节比对。
    async fn apply(context: Context, config: ProbeConfig) -> Result<()> {
        let data: Bytes = (0..1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>()
            .into();
        let expected = Bundle {
            record_value: Part { data: data.clone() },
            variant_value: Selection::Data(data.clone()),
            map_value: [("bytes".into(), data.clone())].into_iter().collect(),
            option_value: Some(data.clone()),
            result_value: Ok(data),
        };
        let result = context
            .get::<dyn Exchange>()?
            .exchange(expected.clone())
            .await?;
        if result != expected {
            return Err(Error::new(1, "聚合字节往返值不同"));
        }
        std::fs::write(
            config.result,
            "{\"bytes_per_field\":1048576,\"fields\":5,\"equal\":true}\n",
        )
        .map_err(|problem| Error::new(1, problem.to_string()))
    }
}

cordis_sdk::export_plugin!(
    name = "http-tests",
    version = "0.1.0",
    components = [
        Provider,
        Probe,
        threads::Provider,
        threads::Probe,
        threads::Idle,
        identity::Provider,
        identity::Probe,
        secrets::Probe,
        secrets::Child,
        streams::Provider,
        streams::Probe,
        conflicts::Probe
    ]
);
