//! DOC-15 §9、CP-9：以 TOML 和声明类型解析清单，插口约束在转换边界完成。

use crate::config::{Result, instance_name, parameter, text};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "RawManifest")]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(flatten)]
    pub entry: Entry,
    #[serde(default)]
    pub web: Option<Web>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    name: String,
    version: String,
    socket: String,
    library: Option<PathBuf>,
    protocol: Option<u32>,
    command: Option<Vec<String>>,
    restart: Option<Restart>,
    web: Option<Web>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "socket", rename_all = "lowercase")]
pub enum Entry {
    Native {
        library: PathBuf,
    },
    Process {
        protocol: u32,
        command: Vec<String>,
        #[serde(default)]
        restart: Restart,
    },
    Web,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Web {
    pub assets: PathBuf,
    pub mount: String,
    pub entry: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Restart {
    pub max: u32,
    pub backoff_ms: u64,
}
impl Default for Restart {
    fn default() -> Self {
        Self {
            max: 5,
            backoff_ms: 500,
        }
    }
}

/// DOC-15 §9：文件字节按 UTF-8 与 TOML 解析，字段和插口错误交由声明类型返回。
pub fn parse(bytes: &[u8]) -> Result<Manifest> {
    toml::from_str(text(bytes)?).map_err(parameter)
}

impl TryFrom<RawManifest> for Manifest {
    type Error = cordis_up::WireError;
    fn try_from(raw: RawManifest) -> Result<Self> {
        instance_name(&raw.name)?;
        semver::Version::parse(&raw.version).map_err(parameter)?;
        let entry = match raw.socket.as_str() {
            "native" => {
                let library = raw
                    .library
                    .filter(|path| !path.as_os_str().is_empty())
                    .ok_or_else(|| parameter("原生插件需要库路径"))?;
                if raw.protocol.is_some() || raw.command.is_some() || raw.restart.is_some() {
                    return Err(parameter("原生插件由宿主管理启动与重启"));
                }
                Entry::Native { library }
            }
            "process" => {
                if raw.protocol != Some(1) {
                    return Err(parameter("进程插件需要 UP/1"));
                }
                let command = raw
                    .command
                    .filter(|command| !command.is_empty() && !command[0].is_empty())
                    .ok_or_else(|| parameter("进程插件需要启动命令"))?;
                if raw.library.is_some() {
                    return Err(parameter("进程插件通过启动命令接入"));
                }
                Entry::Process {
                    protocol: 1,
                    command,
                    restart: raw.restart.unwrap_or_default(),
                }
            }
            "web" => {
                if raw.web.is_none() {
                    return Err(parameter("网页插件需要 web 声明"));
                }
                if raw.library.is_some()
                    || raw.protocol.is_some()
                    || raw.command.is_some()
                    || raw.restart.is_some()
                {
                    return Err(parameter("网页插件通过网页宿主接入"));
                }
                Entry::Web
            }
            _ => return Err(parameter("socket 需要 native、process 或 web")),
        };
        Ok(Self {
            name: raw.name,
            version: raw.version,
            entry,
            web: raw.web,
        })
    }
}
