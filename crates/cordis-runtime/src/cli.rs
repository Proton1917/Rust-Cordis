//! DOC-15 §3.1、§5.1：核心命令、开发工具转交与系统服务配置。

use crate::{
    config::{self, Result, parameter},
    storage::{self, io_error},
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
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
    Run(ConfigurationPath),
    /// 读取项目状态文件。
    Status(ConfigurationPath),
    /// 生成并安装系统服务配置。
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

#[derive(Subcommand)]
pub enum ServiceAction {
    Install(ServiceOptions),
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ServicePlatform {
    Launchd,
    Systemd,
}

#[derive(Args)]
pub struct ServiceOptions {
    #[arg(short, long, default_value = "cordis.toml")]
    pub config: PathBuf,
    #[arg(long, default_value = "com.cordis.runtime")]
    pub name: String,
    #[arg(long, value_enum)]
    pub platform: Option<ServicePlatform>,
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// 只生成服务配置。
    #[arg(long)]
    pub dry_run: bool,
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
    let configuration = configuration.canonicalize().map_err(io_error)?;
    let parsed = config::parse(&storage::read(&configuration)?)?;
    let directory = config::resolve_path(
        configuration.parent().expect("configuration parent"),
        &parsed.runtime.state_dir,
    );
    let value: serde_json::Value =
        serde_json::from_slice(&storage::read(&directory.join("status.json"))?)
            .map_err(parameter)?;
    serde_json::to_string_pretty(&value).map_err(parameter)
}

/// DOC-15 §5.1：生成项目内服务文件，dry_run 保留文件供检查，其余调用系统服务管理器。
pub fn service(options: ServiceOptions) -> Result<PathBuf> {
    if options.name.is_empty()
        || !options
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(parameter("服务名称需要字母、数字、点、短横线或下划线"));
    }
    let configuration = options.config.canonicalize().map_err(io_error)?;
    config::parse(&storage::read(&configuration)?)?;
    let project = configuration.parent().expect("configuration parent");
    let binary = env::current_exe().map_err(io_error)?;
    let platform = options.platform.unwrap_or(if cfg!(target_os = "macos") {
        ServicePlatform::Launchd
    } else {
        ServicePlatform::Systemd
    });
    let generated = service_template(platform, &options.name, &binary, &configuration, project);
    fs::create_dir_all(project.join("service")).map_err(io_error)?;
    let suffix = match platform {
        ServicePlatform::Launchd => "plist",
        ServicePlatform::Systemd => "service",
    };
    let filename = format!("{}.{suffix}", options.name);
    let output = options
        .output
        .unwrap_or_else(|| project.join("service").join(filename));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    storage::write(&output, generated.as_bytes())?;
    if !options.dry_run {
        install_service(platform, &output.canonicalize().map_err(io_error)?)?;
    }
    Ok(output)
}

/// DOC-15 §5.1：两个平台的服务管理器使用相同核心命令与项目工作目录。
fn service_template(
    platform: ServicePlatform,
    name: &str,
    binary: &Path,
    configuration: &Path,
    project: &Path,
) -> String {
    match platform {
        ServicePlatform::Launchd => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>run</string><string>--config</string><string>{}</string></array>\n<key>WorkingDirectory</key><string>{}</string>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>StandardOutPath</key><string>{}</string>\n<key>StandardErrorPath</key><string>{}</string>\n</dict></plist>\n",
            xml(name),
            xml_path(binary),
            xml_path(configuration),
            xml_path(project),
            xml_path(&project.join("service/stdout.log")),
            xml_path(&project.join("service/stderr.log"))
        ),
        ServicePlatform::Systemd => format!(
            "[Unit]\nDescription=Cordis {}\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart={} run --config {}\nRestart=always\nRestartSec=1\nKillMode=control-group\n\n[Install]\nWantedBy=default.target\n",
            name,
            unit(project),
            unit(binary),
            unit(configuration)
        ),
    }
}

/// DOC-15 §5.1：将已生成配置交给系统管理器，平台命令失败按原退出状态报告。
fn install_service(platform: ServicePlatform, output: &Path) -> Result<()> {
    let result = match platform {
        ServicePlatform::Launchd => {
            let user = Command::new("id").arg("-u").output().map_err(io_error)?;
            if !user.status.success() {
                return Err(cordis_up::WireError::new(1, "读取服务运行用户失败"));
            }
            let uid = std::str::from_utf8(&user.stdout).map_err(parameter)?.trim();
            let domain = if uid == "0" {
                "system".to_owned()
            } else {
                format!("gui/{uid}")
            };
            Command::new("launchctl")
                .arg("bootstrap")
                .arg(domain)
                .arg(output)
                .status()
                .map_err(io_error)?
        }
        ServicePlatform::Systemd => {
            let linked = Command::new("systemctl")
                .args(["--user", "link"])
                .arg(output)
                .status()
                .map_err(io_error)?;
            if !linked.success() {
                return Err(cordis_up::WireError::new(
                    1,
                    format!("systemctl link 退出：{linked}"),
                ));
            }
            Command::new("systemctl")
                .args(["--user", "enable", "--now"])
                .arg(output.file_name().expect("service filename"))
                .status()
                .map_err(io_error)?
        }
    };
    if !result.success() {
        return Err(cordis_up::WireError::new(
            1,
            format!("服务安装命令退出：{result}"),
        ));
    }
    Ok(())
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn xml_path(path: &Path) -> String {
    xml(&path.to_string_lossy())
}
fn unit(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    )
}
