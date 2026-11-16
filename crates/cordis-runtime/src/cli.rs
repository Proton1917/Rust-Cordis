//! DOC-15 §3.1、§5.1：核心命令、开发工具转交与系统服务配置。

use crate::{
    config::{self, Result, parameter},
    storage::{self, io_error},
};
use clap::{Args, Parser, Subcommand};
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

mod service;
pub use service::{
    ServiceAction, ServiceOptions, ServicePlatform, UninstallOptions, service, uninstall,
};

#[derive(Parser)]
#[command(name = "cordis", version, about = "可组合组件运行时")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Action>,
}

#[derive(Subcommand)]
pub enum Action {
    /// 按项目配置启动核心与插件进程。
    Run(RunOptions),
    /// 读取运行快照或已确认程序绑定。
    Status(StatusOptions),
    /// 生成、安装或卸载系统服务配置。
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// 显示核心版本。
    Version,
    #[command(external_subcommand)]
    Developer(Vec<OsString>),
}

#[derive(Args)]
pub struct ConfigurationPath {
    #[arg(short, long, default_value = "cordis.toml")]
    pub config: PathBuf,
}

#[derive(Args)]
pub struct StatusOptions {
    #[command(flatten)]
    pub configuration: ConfigurationPath,
    /// 输出带版本的已确认程序绑定，适用于运行期间和停机后。
    #[arg(long)]
    pub offline: bool,
}

impl StatusOptions {
    /// ADR-0054：离线模式读取已确认绑定，普通模式继续读取运行快照。
    pub fn read(self) -> Result<String> {
        if self.offline {
            offline_status(&self.configuration.config)
        } else {
            status(&self.configuration.config)
        }
    }
}

#[derive(Args)]
pub struct RunOptions {
    #[arg(short, long, default_value = "cordis.toml")]
    pub config: PathBuf,
    #[arg(long, requires_all = ["contract_subject", "contract_report"])]
    pub contract_suite: Option<String>,
    #[arg(long, requires_all = ["contract_suite", "contract_report"])]
    pub contract_subject: Option<String>,
    #[arg(long, requires_all = ["contract_suite", "contract_subject"])]
    pub contract_report: Option<PathBuf>,
    #[arg(long, default_value_t = 30_000)]
    pub contract_timeout_ms: u64,
}

impl RunOptions {
    /// ADR-0017：契约参数成组建立独立会话，普通 run 沿既有部署流程。
    pub async fn run(self) -> Result<()> {
        let plan = match (
            self.contract_suite,
            self.contract_subject,
            self.contract_report,
        ) {
            (Some(suite), Some(subject), Some(report)) if self.contract_timeout_ms > 0 => {
                Some(crate::contract::Plan {
                    suite,
                    subject,
                    report,
                    timeout: std::time::Duration::from_millis(self.contract_timeout_ms),
                })
            }
            (None, None, None) => None,
            _ => {
                return Err(parameter(
                    "契约会话需要套件、被测实例、报告路径与正截止时间",
                ));
            }
        };
        crate::run_with_contract(self.config, plan).await
    }
}

/// DOC-15 §3.1：优先使用同目录程序，其次查 PATH，缺席时返回可读错误。
pub fn executable(name: &str) -> Result<PathBuf> {
    let current = env::current_exe().map_err(io_error)?;
    let sibling = current.parent().expect("executable parent").join(name);
    if sibling.is_file() {
        return Ok(sibling);
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(cordis_up::WireError::new(
        3,
        format!("程序 {name} 安装路径缺席"),
    ))
}

/// DOC-15 §3.1：参数与退出码交由独立开发工具处理，核心保持运行功能边界。
pub fn forward(arguments: &[OsString]) -> Result<i32> {
    let tool = executable("cordis-dev").map_err(|_| {
        cordis_up::WireError::new(3, "该命令由开发工具 cordis-dev 提供，当前未安装")
    })?;
    let status = Command::new(tool)
        .args(arguments)
        .status()
        .map_err(io_error)?;
    use std::os::unix::process::ExitStatusExt;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
}

/// DOC-11 §10：按配置定位已发布状态文件，读取和 JSON 格式错误直接返回。
pub fn status(configuration: &Path) -> Result<String> {
    let (_, directory) = status_location(configuration)?;
    let value: serde_json::Value =
        serde_json::from_slice(&storage::read(&directory.join("status.json"))?)
            .map_err(parameter)?;
    serde_json::to_string_pretty(&value).map_err(parameter)
}

/// ADR-0054：公开最近持久确认的绑定，读取失败直接返回，路径按原记录展开。
pub fn offline_status(configuration: &Path) -> Result<String> {
    let (project, directory) = status_location(configuration)?;
    let snapshot = crate::control::offline_sources(&directory.join("sources.json"), &project)?;
    serde_json::to_string_pretty(&snapshot).map_err(parameter)
}

/// ADR-0036、ADR-0054：两种查询共用配置与应用状态目录解析。
fn status_location(configuration: &Path) -> Result<(PathBuf, PathBuf)> {
    let configuration = configuration.canonicalize().map_err(io_error)?;
    let parsed = crate::deployment::load(&configuration)?;
    let project = configuration.parent().expect("configuration parent");
    let directory = config::resolve_path(project, &parsed.runtime.state_dir);
    Ok((project.to_owned(), directory))
}

/// ADR-0036：Finder 启动沿可执行文件定位 Bundle 部署，命令行沿当前目录。
pub async fn run_default() -> Result<()> {
    crate::run(crate::deployment::default_configuration()?).await
}
