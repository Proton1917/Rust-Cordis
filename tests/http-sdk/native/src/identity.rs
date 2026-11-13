//! ADR-0046 AU-25：原生 Scoped 服务查询页面属性，后端调用保持自己的来源身份。

use cordis_sdk::prelude::*;
use http_api::HttpPage;

#[cordis_sdk::interface(name = "test.identity", version = "1.0")]
trait Identity: Send + Sync {
    async fn inspect(&self) -> Result<String>;
}

struct View {
    context: Context,
    meta: CborValue,
}
#[cordis_sdk::interface]
impl Identity for View {
    async fn inspect(&self) -> Result<String> {
        let attributes = http_api::page_attributes(&self.context, &self.meta).await?;
        let secret = match self.context.get::<dyn secrets_api::Secrets>() {
            Ok(service) => service.get("ALPHA".into()).await?,
            Err(error) if error.code() == 4 => None,
            Err(error) => return Err(error),
        };
        if let Some(value) = &secret {
            let path = self.context.data_dir()?.join("backend-secret.json");
            std::fs::write(
                path,
                serde_json::to_vec(
                    &serde_json::json!({"value":value,"requested_by_page":attributes.is_some()}),
                )
                .map_err(problem)?,
            )
            .map_err(problem)?;
        }
        let report = serde_json::json!({"metadata": self.meta, "attributes": attributes, "secret_obtained": secret.is_some()});
        serde_json::to_string(&report).map_err(problem)
    }
}

pub struct Provider;
impl Component for Provider {
    const NAME: &'static str = "identity-provider";
    type Config = EmptyConfig;
    fn inject() -> Inject {
        Inject::new()
            .with::<dyn HttpPage>()
            .with::<dyn cordis_sdk::builtin::DataDir>()
            .optional::<dyn secrets_api::Secrets>()
    }
    fn provides() -> Provides {
        Provides::new().with::<dyn Identity>()
    }
    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        let base: Arc<dyn Identity> = Arc::new(View {
            context: context.clone(),
            meta: CborValue::Map(Vec::new()),
        });
        context.provide_scoped::<dyn Identity>(Scoped::new(base, {
            let context = context.clone();
            move |meta: CborValue| {
                Ok(Arc::new(View {
                    context: context.clone(),
                    meta,
                }) as Arc<dyn Identity>)
            }
        }))
    }
}

#[derive(serde::Deserialize)]
pub struct Config {
    result: String,
}

pub struct Probe;
impl Component for Probe {
    const NAME: &'static str = "identity-probe";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new().with::<dyn Identity>()
    }
    async fn apply(context: Context, config: Config) -> Result<()> {
        let result = context.get::<dyn Identity>()?.inspect().await?;
        std::fs::write(config.result, result).map_err(problem)
    }
}

fn problem(problem: impl std::fmt::Display) -> Error {
    Error::new(1, problem.to_string())
}
