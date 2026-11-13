//! DOC-15 §5.1：服务文件持久安装、生成与卸载，系统管理器失败沿命令返回。

use crate::{
    config::{self, Result, parameter},
    storage::{self, io_error},
};
use clap::{Args, Subcommand, ValueEnum};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

mod template;

#[derive(Subcommand)]
pub enum ServiceAction {
    /// 生成配置并注册为登录或系统启动服务。
    Install(ServiceOptions),
    /// 停止服务并删除安装配置。
    Uninstall(UninstallOptions),
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ServicePlatform {
    Launchd,
    Systemd,
}

impl Default for ServicePlatform {
    fn default() -> Self {
        if cfg!(target_os = "macos") {
            Self::Launchd
        } else {
            Self::Systemd
        }
    }
}

impl ServicePlatform {
    fn filename(self, name: &str) -> String {
        let suffix = match self {
            Self::Launchd => "plist",
            Self::Systemd => "service",
        };
        format!("{name}.{suffix}")
    }
}

#[derive(Args)]
pub struct ServiceOptions {
    #[arg(short, long, default_value = "cordis.toml")]
    pub config: PathBuf,
    #[arg(long, default_value = "com.cordis.runtime")]
    pub name: String,
    #[arg(long, value_enum)]
    pub platform: Option<ServicePlatform>,
    /// 在指定位置生成配置文件。
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// 在项目 service 目录生成配置，位置可由 --output 指定。
    #[arg(long)]
    pub dry_run: bool,
    /// 为服务管理器设置一个非机密环境变量，可重复指定。
    #[arg(long = "env", value_name = "NAME=VALUE")]
    pub environment: Vec<String>,
    /// 从当前进程继承指定的非机密环境变量，可重复指定。
    #[arg(long, value_name = "NAME")]
    pub inherit_env: Vec<String>,
}

#[derive(Args)]
pub struct UninstallOptions {
    #[arg(long)]
    pub name: String,
    #[arg(long, value_enum)]
    pub platform: Option<ServicePlatform>,
}

/// DOC-15 §5.1：默认采用系统读取的安装目录；显式输出与 dry_run 生成可检查文件。
pub fn service(options: ServiceOptions) -> Result<PathBuf> {
    validate_name(&options.name)?;
    let environment = service_environment(&options.environment, &options.inherit_env)?;
    let configuration = options.config.canonicalize().map_err(io_error)?;
    config::parse(&storage::read(&configuration)?)?;
    let project = configuration.parent().expect("configuration parent");
    let binary = env::current_exe().map_err(io_error)?;
    let platform = options.platform.unwrap_or_default();
    let generated = template::render(
        platform,
        &options.name,
        &binary,
        &configuration,
        project,
        &environment,
    );
    let installation = if options.output.is_none() && !options.dry_run {
        Some(Location::current(platform, &options.name)?)
    } else {
        None
    };
    let output = options.output.unwrap_or_else(|| {
        installation
            .as_ref()
            .map(|location| location.path.clone())
            .unwrap_or_else(|| {
                project
                    .join("service")
                    .join(platform.filename(&options.name))
            })
    });
    fs::create_dir_all(project.join("service")).map_err(io_error)?;
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    storage::write(&output, generated.as_bytes())?;
    if let Some(location) = installation {
        location.install()?;
    }
    Ok(output)
}

/// DOC-15 §5.1：先确认该服务的安装文件，再停止管理器中的任务，最后删除同一路径。
pub fn uninstall(options: UninstallOptions) -> Result<PathBuf> {
    validate_name(&options.name)?;
    Location::current(options.platform.unwrap_or_default(), &options.name)?.uninstall()
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(parameter("服务名称需要字母、数字、点、短横线或下划线"));
    }
    Ok(())
}

/// ADR-0046 §3.10：服务安装只收取显式命名的非机密变量，配置写入前完成分类。
fn service_environment(
    values: &[String],
    inherited: &[String],
) -> Result<BTreeMap<String, String>> {
    let mut environment = BTreeMap::new();
    for name in inherited {
        environment_name(name)?;
        let value = env::var(name).map_err(|_| parameter(format!("读取环境变量 {name} 失败")))?;
        environment.insert(name.clone(), value);
    }
    for value in values {
        let (name, value) = value
            .split_once('=')
            .ok_or_else(|| parameter("环境变量参数需要 NAME=VALUE"))?;
        environment_name(name)?;
        environment.insert(name.into(), value.into());
    }
    Ok(environment)
}

fn environment_name(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(parameter(
            "环境变量名称需要字母、数字与下划线，并以字母或下划线开头",
        ));
    }
    let normalized = name.to_ascii_uppercase();
    if ["SECRET", "TOKEN", "PASSWORD", "KEY"]
        .iter()
        .any(|part| normalized.contains(part))
    {
        return Err(parameter(format!(
            "变量 {name} 属于机密命名，请通过 secrets 插件提供"
        )));
    }
    Ok(())
}

struct Location {
    platform: ServicePlatform,
    uid: u32,
    path: PathBuf,
}

impl Location {
    fn current(platform: ServicePlatform, name: &str) -> Result<Self> {
        let user = Command::new("id").arg("-u").output().map_err(io_error)?;
        if !user.status.success() {
            return Err(cordis_up::WireError::new(1, "读取服务运行用户失败"));
        }
        let uid = std::str::from_utf8(&user.stdout)
            .map_err(parameter)?
            .trim()
            .parse::<u32>()
            .map_err(parameter)?;
        let home = if uid == 0 && matches!(platform, ServicePlatform::Launchd) {
            PathBuf::new()
        } else {
            env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| parameter("服务用户的 HOME 路径缺席"))?
        };
        let config_home = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let path = installation_path(platform, name, uid, &home, config_home.as_deref());
        Ok(Self {
            platform,
            uid,
            path,
        })
    }

    fn domain(&self) -> String {
        if self.uid == 0 {
            "system".into()
        } else {
            format!("gui/{}", self.uid)
        }
    }

    /// DOC-15 §5.1：在持久目录写入后启动任务，管理器报告决定安装结果。
    fn install(&self) -> Result<()> {
        match self.platform {
            ServicePlatform::Launchd => run(Command::new("launchctl")
                .arg("bootstrap")
                .arg(self.domain())
                .arg(&self.path)),
            ServicePlatform::Systemd => {
                run(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
                run(Command::new("systemctl")
                    .args(["--user", "enable", "--now"])
                    .arg(self.path.file_name().expect("service filename")))
            }
        }
    }

    /// DOC-15 §5.1：缺席或管理器拒绝时保留现有文件，成功停止后删除安装文件。
    fn uninstall(self) -> Result<PathBuf> {
        fs::symlink_metadata(&self.path).map_err(|error| {
            cordis_up::WireError::new(
                1,
                format!("读取服务安装文件 {}：{error}", self.path.display()),
            )
        })?;
        match self.platform {
            ServicePlatform::Launchd => run(Command::new("launchctl")
                .arg("bootout")
                .arg(self.domain())
                .arg(&self.path))?,
            ServicePlatform::Systemd => run(Command::new("systemctl")
                .args(["--user", "disable", "--now"])
                .arg(self.path.file_name().expect("service filename")))?,
        }
        fs::remove_file(&self.path).map_err(io_error)?;
        Ok(self.path)
    }
}

/// DOC-15 §5.1：路径计算可独立验证；macOS root 使用系统目录，Linux 遵守 XDG 配置目录。
fn installation_path(
    platform: ServicePlatform,
    name: &str,
    uid: u32,
    home: &Path,
    config_home: Option<&Path>,
) -> PathBuf {
    let directory = match platform {
        ServicePlatform::Launchd if uid == 0 => PathBuf::from("/Library/LaunchDaemons"),
        ServicePlatform::Launchd => home.join("Library/LaunchAgents"),
        ServicePlatform::Systemd => config_home
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".config"))
            .join("systemd/user"),
    };
    directory.join(platform.filename(name))
}

fn run(command: &mut Command) -> Result<()> {
    let output = command.output().map_err(io_error)?;
    if !output.status.success() {
        return Err(cordis_up::WireError::new(
            1,
            format!(
                "服务管理命令退出 {}：{}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
