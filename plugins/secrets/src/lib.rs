//! ADR-0046 §3.10/3.11：插件读取密钥文件，并按核心给出的实例身份执行自身授权策略。
#![forbid(unsafe_code)]

use cordis_sdk::prelude::*;
use secrets_api::Secrets;
use serde::Deserialize;
use std::{collections::BTreeMap, fs::File, io::Read, os::unix::fs::MetadataExt, path::PathBuf};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub file: PathBuf,
    #[serde(default)]
    pub grants: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(rename = "cordis.caller")]
    caller: Option<Caller>,
}
#[derive(Deserialize)]
struct Caller {
    instance: Option<String>,
}

struct Store {
    values: Arc<BTreeMap<String, String>>,
    grants: Arc<BTreeMap<String, Vec<String>>>,
    caller: Option<String>,
}

#[cordis_sdk::interface]
impl Secrets for Store {
    async fn get(&self, name: String) -> Result<Option<String>> {
        let allowed = self
            .caller
            .as_ref()
            .and_then(|caller| self.grants.get(caller));
        if !allowed.is_some_and(|allowed| allowed.contains(&name)) {
            return Err(Error::new(9, "当前实例缺少该密钥的读取授权"));
        }
        Ok(self.values.get(&name).cloned())
    }
}

struct Provider;
impl Component for Provider {
    const NAME: &'static str = "secrets";
    type Config = Config;
    fn provides() -> Provides {
        Provides::new().with::<dyn Secrets>()
    }

    /// ADR-0046 §3.10：同一打开文件上检查属主和权限，再由 dotenv-parser 读取字面值。
    async fn apply(context: Context, config: Config) -> Result<()> {
        let path = config.file.display();
        let failure = |problem| Error::new(1, format!("读取密钥文件 {path}：{problem}"));
        let mut file = File::open(&config.file).map_err(failure)?;
        let info = file.metadata().map_err(failure)?;
        if info.uid() != rustix::process::geteuid().as_raw() || info.mode() & 0o7777 & !0o600 != 0 {
            return Err(Error::new(
                9,
                format!(
                    "密钥文件 {path} 属主 {}、权限 {:04o}；要求当前用户及至多 0600",
                    info.uid(),
                    info.mode() & 0o7777
                ),
            ));
        }
        let mut source = String::new();
        file.read_to_string(&mut source).map_err(failure)?;
        let values = Arc::new(
            dotenv_parser::parse_dotenv(&source)
                .map_err(|_| Error::new(2, format!("密钥文件 {path} 的 NAME=VALUE 格式错误")))?,
        );
        let grants = Arc::new(config.grants);
        let base: Arc<dyn Secrets> = Arc::new(Store {
            values: values.clone(),
            grants: grants.clone(),
            caller: None,
        });
        context.provide_scoped::<dyn Secrets>(Scoped::new(base, move |meta: Metadata| {
            Ok(Arc::new(Store {
                values: values.clone(),
                grants: grants.clone(),
                caller: meta.caller.and_then(|caller| caller.instance),
            }) as Arc<dyn Secrets>)
        }))
    }
}

cordis_sdk::export_plugin!(
    name = "secrets",
    version = env!("CARGO_PKG_VERSION"),
    components = [Provider]
);
