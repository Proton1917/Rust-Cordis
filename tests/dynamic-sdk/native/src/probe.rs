//! AU-27/28/29/30：两种代理对同一状态执行全部 KV 方法，持续读取随依赖生命周期结束。

use super::{Identity, Undeclared, problem};
use cordis_sdk::prelude::*;
use kv_api::Kv;
use std::{fs, path::Path, time::Duration};

#[derive(serde::Deserialize)]
pub struct Config {
    result: String,
    stream: bool,
}

pub struct Probe;
impl Component for Probe {
    const NAME: &'static str = "probe";
    type Config = Config;
    fn inject() -> Inject {
        Inject::new()
            .with_name("kv@1.0")
            .with::<dyn Identity>()
            .optional_name("dynamic.streams@1.0")
    }
    async fn apply(context: Context, config: Config) -> Result<()> {
        let typed = context.get::<dyn Kv>()?;
        let dynamic = context.get_dynamic("kv@1.0")?;
        compare(&typed, &dynamic).await?;
        let errors = errors(&context, &dynamic)?;
        let metadata = identity(&context)?;
        let resources = if config.stream {
            Some(super::streams::exercise(&context).await?)
        } else {
            None
        };
        let revision = context.context_id();
        let result = serde_json::json!({"context":revision,"typed":typed.clone_object()?.has_typed_methods(),"errors":errors,"metadata":metadata,"resources":resources});
        write(Path::new(&config.result), &result)?;
        let task_context = context.clone();
        context.spawn(async move {
            let mut iteration = 0u64;
            loop {
                let first = typed.get("dynamic-value")?;
                let second: Option<Bytes> = dynamic.invoke_as("get", &("dynamic-value",))?;
                if first != second.map(Vec::from) {
                    return Err(Error::new(1, "动态与类型化读取结果不同"));
                }
                iteration += 1;
                if iteration % 10 == 0 {
                    let mut record = result.clone();
                    record["iterations"] = iteration.into();
                    write(Path::new(&config.result), &record)?;
                }
                task_context.sleep(Duration::from_millis(20)).await?;
            }
        })
    }
}

async fn compare(typed: &Service<dyn Kv>, dynamic: &DynamicService) -> Result<()> {
    if typed.get("missing")?.is_some()
        || dynamic.invoke("get", CborValue::Array(vec!["missing".into()]))? != CborValue::Null
    {
        return Err(Error::new(1, "缺席键应返回空值"));
    }
    typed.put("typed-value", vec![0, 128, 255])?;
    let received: Option<Bytes> = dynamic.invoke_as("get", &("typed-value",))?;
    if received.map(Vec::from) != typed.get("typed-value")? {
        return Err(Error::new(1, "动态读取与类型化写入不同"));
    }
    dynamic.invoke_as::<_, ()>("put", &("dynamic-value", Bytes(vec![3, 5, 7])))?;
    if typed.get("dynamic-value")? != Some(vec![3, 5, 7]) {
        return Err(Error::new(1, "类型化读取与动态写入不同"));
    }
    let keys: Vec<String> = dynamic.invoke_as("keys", &Vec::<CborValue>::new())?;
    if keys != typed.keys()? {
        return Err(Error::new(1, "两种代理的键集合不同"));
    }
    typed.flush().await?;
    dynamic
        .invoke_async("flush", CborValue::Array(Vec::new()))
        .await?;
    Ok(())
}

fn access(context: &Context) -> Result<serde_json::Value> {
    let typed = context
        .get::<dyn Undeclared>()
        .err()
        .ok_or_else(|| Error::new(1, "未声明依赖取得成功"))?;
    let dynamic = context
        .get_dynamic("dynamic.undeclared@1.0")
        .err()
        .ok_or_else(|| Error::new(1, "动态未声明依赖取得成功"))?;
    if typed.code() != dynamic.code() {
        return Err(Error::new(1, "依赖错误类别不同"));
    }
    let secret_typed = context
        .get::<dyn secrets_api::Secrets>()
        .err()
        .ok_or_else(|| Error::new(1, "未声明密钥依赖取得成功"))?;
    let secret_dynamic = context
        .get_dynamic("cordis.secrets@1.0")
        .err()
        .ok_or_else(|| Error::new(1, "动态未声明密钥依赖取得成功"))?;
    if secret_typed.code() != secret_dynamic.code() {
        return Err(Error::new(1, "密钥依赖错误类别不同"));
    }
    context.get::<dyn cordis_sdk::builtin::Runtime>()?;
    context.get_dynamic("cordis.runtime@1.3")?;
    Ok(
        serde_json::json!({"undeclared":dynamic.code(),"typed_undeclared":typed.code(),"undeclared_secrets":secret_dynamic.code(),"typed_undeclared_secrets":secret_typed.code(),"builtin_runtime":true}),
    )
}

fn errors(context: &Context, service: &DynamicService) -> Result<serde_json::Value> {
    let mut result = access(context)?;
    let mut records = Vec::new();
    for (method, args, expected) in [
        ("absent-method", Vec::new(), 3),
        ("get", Vec::new(), 2),
        ("get", vec![true.into()], 2),
        (
            "put",
            vec![
                "forged".into(),
                CborValue::Tag(60000, Box::new(1u64.into())),
            ],
            2,
        ),
    ] {
        let error = service
            .invoke(method, CborValue::Array(args))
            .err()
            .ok_or_else(|| Error::new(1, "错误调用取得成功"))?;
        if error.code() != expected {
            return Err(error);
        }
        records.push(serde_json::json!({"method":method,"code":error.code()}));
    }
    result["calls"] = records.into();
    Ok(result)
}

fn identity(context: &Context) -> Result<serde_json::Value> {
    let typed = context.get::<dyn Identity>()?.metadata()?;
    let dynamic = context
        .get_dynamic("dynamic.identity@1.0")?
        .invoke("metadata", CborValue::Array(Vec::new()))?;
    if typed != dynamic {
        return Err(Error::new(1, "两种代理的可信身份不同"));
    }
    typed.deserialized().map_err(problem)
}

fn write(path: &Path, value: &serde_json::Value) -> Result<()> {
    let temporary = path.with_extension("next");
    fs::write(&temporary, serde_json::to_vec(value).map_err(problem)?).map_err(problem)?;
    fs::rename(temporary, path).map_err(problem)
}
