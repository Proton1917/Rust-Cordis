//! DOC-15 §12.2、ADR-0036：发布源保持原位，应用的状态、安装副本与宿主缓存使用可写目录。

use crate::{
    config::{self, Configuration, Result, parameter},
    storage::{self, io_error},
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

pub(crate) struct Locations {
    pub sources: PathBuf,
    pub installed: PathBuf,
    pub cache: PathBuf,
    pub verified: Vec<PathBuf>,
    pub app: bool,
}

/// ADR-0036：默认入口来自当前 Bundle，普通命令行部署继续读取当前目录。
pub(crate) fn default_configuration() -> Result<PathBuf> {
    Ok(bundle_configuration(&env::current_exe().map_err(io_error)?)
        .unwrap_or_else(|| "cordis.toml".into()))
}

fn bundle_configuration(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app")
        .then(|| contents.join("Resources/cordis.toml"))
}

/// ADR-0036：启动、status 和 reload 共用应用路径解析，TOML 语法继续由 config::parse 处理。
pub(crate) fn load(path: &Path) -> Result<Configuration> {
    let mut configuration = config::parse(&storage::read(path)?)?;
    if is_app(&configuration)? && configuration.runtime.state_dir == Path::new("state") {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| parameter("应用状态需要用户 HOME 路径"))?;
        configuration.runtime.state_dir = application_state(&configuration, &home)?;
    }
    Ok(configuration)
}

fn is_app(configuration: &Configuration) -> Result<bool> {
    match configuration.pack.get("app") {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| parameter("pack.app 需要布尔值")),
        None => Ok(false),
    }
}

fn application_state(configuration: &Configuration, home: &Path) -> Result<PathBuf> {
    let identifier = configuration
        .pack
        .get("identifier")
        .and_then(toml::Value::as_str)
        .filter(|identifier| {
            !identifier.is_empty()
                && *identifier != "."
                && *identifier != ".."
                && identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        })
        .ok_or_else(|| parameter("app 需要有效的 pack.identifier"))?;
    Ok(home.join("Library/Application Support").join(identifier))
}

impl Locations {
    /// ADR-0036：app 只读取资源目录，写入集中在 state_dir；普通部署保留原目录规则。
    pub(crate) fn create(project: &Path, configuration: &Configuration) -> Result<Self> {
        let app = is_app(configuration)?;
        let sources = config::resolve_path(project, &configuration.runtime.plugin_dir);
        if !app {
            fs::create_dir_all(&sources).map_err(io_error)?;
        }
        let sources = sources.canonicalize().map_err(io_error)?;
        let work = if app {
            config::resolve_path(project, &configuration.runtime.state_dir)
        } else {
            project.to_owned()
        };
        if app && work.starts_with(project.parent().unwrap_or(project)) {
            return Err(parameter("app 状态目录需要位于 Contents 之外"));
        }
        let installed = if app {
            work.join("plugins")
        } else {
            sources.clone()
        };
        fs::create_dir_all(&installed).map_err(io_error)?;
        let installed = installed.canonicalize().map_err(io_error)?;
        let cache = work.join(".cordis/lib");
        let mut verified = Vec::new();
        if app {
            let bundled = project.join(".cordis/verified");
            if bundled.is_dir() {
                verified.push(bundled);
            }
            let writable = work.join(".cordis/verified");
            fs::create_dir_all(&writable).map_err(io_error)?;
            verified.push(writable);
        } else {
            verified.push(project.join(".cordis/verified"));
        }
        Ok(Self {
            sources,
            installed,
            cache,
            verified,
            app,
        })
    }

    /// DOC-11 SW-2：每个验证根都是显式部署位置，Host 解析印记时约束在这些目录内。
    pub(crate) fn verification_arguments(&self, command: &mut Vec<String>) {
        command.push("--require-verified".into());
        for directory in &self.verified {
            command.extend([
                "--verified-dir".into(),
                directory.to_string_lossy().into_owned(),
            ]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_entry_and_application_state_are_independent_of_working_directory() {
        let executable = Path::new("/Applications/Demo.app/Contents/MacOS/cordis");
        assert_eq!(
            bundle_configuration(executable),
            Some(PathBuf::from(
                "/Applications/Demo.app/Contents/Resources/cordis.toml"
            ))
        );
        assert_eq!(
            bundle_configuration(Path::new("/usr/local/bin/cordis")),
            None
        );
        let configuration =
            config::parse(b"[pack]\napp=true\nidentifier='org.cordis.demo'\n").unwrap();
        assert_eq!(
            application_state(&configuration, Path::new("/Users/application-owner")).unwrap(),
            PathBuf::from("/Users/application-owner/Library/Application Support/org.cordis.demo")
        );
    }
}
