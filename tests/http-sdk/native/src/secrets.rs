//! ADR-0046 AU-26：部署授予的名称、拒绝结果与子组件继承都经真实服务调用检查。

use cordis_sdk::prelude::*;
use secrets_api::Secrets;

#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub struct Config {
    names: Vec<String>,
    result: String,
    #[serde(default)]
    child_result: Option<String>,
}

async fn inspect(context: &Context, config: &Config) -> Result<()> {
    let service = context.get::<dyn Secrets>()?;
    let mut results = serde_json::Map::new();
    for name in &config.names {
        let result = match service.get(name.clone()).await {
            Ok(value) => serde_json::json!({"value": value}),
            Err(error) => serde_json::json!({"code": error.code(), "message": error.message()}),
        };
        results.insert(name.clone(), result);
    }
    let forged = context
        .intercept::<dyn Secrets, _>(&serde_json::json!({"cordis.caller":{"instance":"reader-b"}}));
    let denied = match forged {
        Err(problem) => problem.code(),
        Ok(_) => return Err(Error::new(1, "伪造调用方身份被接受")),
    };
    if denied != 9 {
        return Err(Error::new(1, "身份伪造需要返回权限错误"));
    }
    let report = serde_json::json!({"results": results, "forged_identity_code": denied});
    std::fs::write(
        &config.result,
        serde_json::to_vec(&report).map_err(problem)?,
    )
    .map_err(problem)
}

pub struct Probe;
impl Component for Probe {
    const NAME: &'static str = "secrets-probe";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new().with::<dyn Secrets>()
    }
    async fn apply(context: Context, config: Config) -> Result<()> {
        inspect(&context, &config).await?;
        if let Some(result) = config.child_result {
            context
                .plugin::<Child>(&Config {
                    names: config.names,
                    result,
                    child_result: None,
                })
                .await?;
        }
        Ok(())
    }
}

pub struct Child;
impl Component for Child {
    const NAME: &'static str = "secrets-child";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new().with::<dyn Secrets>()
    }
    async fn apply(context: Context, config: Config) -> Result<()> {
        inspect(&context, &config).await
    }
}

fn problem(problem: impl std::fmt::Display) -> Error {
    Error::new(1, problem.to_string())
}
