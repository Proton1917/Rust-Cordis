//! DOC-15 §4、§9：安装引用、原生描述符探测与插件进程分组。

use crate::{
    cli,
    config::{Result, RuntimeSettings, Unload, parameter},
    manifest::{self, Entry},
    storage::{self, io_error},
};
use cordis_supervisor::{
    PeerOptions,
    manager::{GroupSpec, RestartPolicy},
};
use cordis_up::payload;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Serialize)]
pub struct Package {
    pub reference: String,
    pub name: String,
    pub version: String,
    pub source: PathBuf,
    pub directory: PathBuf,
    pub entry: Entry,
    pub descriptor: Option<payload::Plugin>,
}

#[derive(Default)]
pub struct Catalog {
    pub packages: BTreeMap<String, Package>,
}

impl Catalog {
    /// DOC-11 §10：按来源建立插件库存，单个来源错误记入诊断并继续其他来源。
    pub async fn discover(
        &mut self,
        directory: &Path,
        project: &Path,
        options: &RuntimeSettings,
    ) -> Result<BTreeMap<String, String>> {
        let mut errors = BTreeMap::new();
        let mut paths = fs::read_dir(directory)
            .map_err(io_error)?
            .map(|entry| entry.map(|entry| entry.path()).map_err(io_error))
            .collect::<Result<Vec<_>>>()?;
        paths.sort();
        for path in paths {
            let Some(reference) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if reference.starts_with('.') || self.packages.contains_key(&reference) {
                continue;
            }
            if path.join("cordis-plugin.toml").is_file() || native(&path) {
                match inspect(reference.clone(), path, project, options).await {
                    Ok(package) => {
                        self.packages.insert(reference, package);
                    }
                    Err(problem) => {
                        errors.insert(reference, problem.to_string());
                    }
                }
            }
        }
        Ok(errors)
    }

    /// DOC-11 §10：优先解析精确引用，逻辑名称存在歧义时返回参数错误。
    pub fn get(&self, reference: &str) -> Result<Package> {
        if let Some(package) = self.packages.get(reference) {
            return Ok(package.clone());
        }
        let mut matches = self.packages.values().filter(|package| {
            package.name == reference && !package.reference.starts_with("installed-")
        });
        let package = matches.next().ok_or_else(|| missing_reference(reference))?;
        if matches.next().is_some() {
            return Err(parameter(format!(
                "插件 {reference} 有多个来源，请在配置中指定文件或目录名"
            )));
        }
        Ok(package.clone())
    }
}

/// ADR-0032：缺席引用同时报告外部构建目录，帮助调用方定位实际插件产物。
fn missing_reference(reference: &str) -> cordis_up::WireError {
    let message = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(directory) => format!(
            "插件引用 {reference} 缺席；CARGO_TARGET_DIR={} 会改变 Cargo 产物位置，请核对 runtime.plugin_dir。脚手架项目使用 cargo build --target-dir target。",
            PathBuf::from(directory).display()
        ),
        None => format!("插件引用 {reference} 缺席"),
    };
    parameter(message)
}

/// DOC-15 Q0、§9：解析清单后通过独立宿主探测原生库，校验声明的名称和版本。
pub async fn inspect(
    reference: String,
    source: PathBuf,
    project: &Path,
    options: &RuntimeSettings,
) -> Result<Package> {
    let source = source.canonicalize().map_err(io_error)?;
    let (declared, directory, entry) = if source.is_dir() {
        let declared = manifest::parse(&storage::read(&source.join("cordis-plugin.toml"))?)?;
        (Some(declared.clone()), source.clone(), declared.entry)
    } else {
        (
            None,
            source.parent().expect("plugin parent").to_owned(),
            Entry::Native {
                library: source.clone(),
            },
        )
    };
    let entry = match entry {
        Entry::Native { library } => Entry::Native {
            library: native_path(&directory, &library)?,
        },
        other => other,
    };
    let descriptor = if let Entry::Native { library } = &entry {
        let descriptor = native_description(library, &directory, project, options).await?;
        if let Some(declared) = &declared
            && (declared.name != descriptor.name || declared.version != descriptor.version)
        {
            return Err(parameter("插件清单与原生描述符的名称或版本冲突"));
        }
        Some(descriptor)
    } else {
        None
    };
    let (name, version) = match (&declared, &descriptor) {
        (Some(declared), _) => (declared.name.clone(), declared.version.clone()),
        (_, Some(descriptor)) => (descriptor.name.clone(), descriptor.version.clone()),
        _ => return Err(parameter("插件来源缺少清单或原生描述符")),
    };
    Ok(Package {
        reference,
        name,
        version,
        source,
        directory,
        entry,
        descriptor,
    })
}

/// DOC-15 Q0、CP-4：描述符来自受限宿主子进程，截止时间和输出上限由进程层执行。
async fn native_description(
    library: &Path,
    directory: &Path,
    project: &Path,
    options: &RuntimeSettings,
) -> Result<payload::Plugin> {
    let host = cli::executable("cordis-host")?;
    let mut command = vec![
        host.to_string_lossy().into_owned(),
        "--describe".into(),
        library.to_string_lossy().into_owned(),
        "--cache-dir".into(),
        project
            .join(".cordis/lib/discovery")
            .to_string_lossy()
            .into_owned(),
    ];
    if options.require_verified {
        command.extend([
            "--require-verified".into(),
            "--verified-dir".into(),
            project
                .join(".cordis/verified")
                .to_string_lossy()
                .into_owned(),
        ]);
    }
    let bytes = cordis_supervisor::run_capture(
        &command,
        directory,
        Duration::from_millis(options.apply_timeout_ms),
        cordis_up::Limits::DEFAULT.max_frame,
    )
    .await?;
    serde_json::from_slice(&bytes).map_err(parameter)
}

/// DOC-15 §4：原生库共享指定宿主，进程插件独占分组，插口冲突在构造命令前返回。
pub fn plan(
    group: &str,
    packages: &[Package],
    project: &Path,
    options: &RuntimeSettings,
) -> Result<GroupSpec> {
    let first = packages
        .first()
        .ok_or_else(|| parameter("分组需要至少一个插件"))?;
    let (command, directory, restart) = match &first.entry {
        Entry::Native { .. } => (
            native_command(group, packages, project, options)?,
            project.to_owned(),
            RestartPolicy {
                max: options.restart_max,
                backoff: Duration::from_millis(options.restart_backoff_ms),
            },
        ),
        Entry::Process {
            command, restart, ..
        } => {
            if packages.len() != 1 {
                return Err(parameter("一个进程插件独占一个分组"));
            }
            (
                command.clone(),
                first.directory.clone(),
                RestartPolicy {
                    max: restart.max,
                    backoff: Duration::from_millis(restart.backoff_ms),
                },
            )
        }
        Entry::Web => return Err(cordis_up::WireError::new(3, "网页插件需要网页宿主连接")),
    };
    Ok(GroupSpec {
        name: group.into(),
        command,
        directory,
        peer: PeerOptions::default(),
        restart,
    })
}

/// DOC-15 §4.1：宿主参数包含该组全部库及生命周期策略，原生库代码由宿主执行。
fn native_command(
    group: &str,
    packages: &[Package],
    project: &Path,
    options: &RuntimeSettings,
) -> Result<Vec<String>> {
    let mut command = vec![
        cli::executable("cordis-host")?
            .to_string_lossy()
            .into_owned(),
        "--cache-dir".into(),
        project
            .join(".cordis/lib")
            .join(group)
            .to_string_lossy()
            .into_owned(),
        "--unload".into(),
        match options.unload {
            Unload::Never => "never",
            Unload::WhenUnleased => "when-unleased",
        }
        .into(),
        "--request-timeout-ms".into(),
        options.invoke_timeout_ms.to_string(),
    ];
    if options.require_verified {
        command.extend([
            "--require-verified".into(),
            "--verified-dir".into(),
            project
                .join(".cordis/verified")
                .to_string_lossy()
                .into_owned(),
        ]);
    }
    for package in packages {
        let Entry::Native { library } = &package.entry else {
            return Err(parameter("一个原生分组需要全部成员使用原生插口"));
        };
        command.extend(["--library".into(), library.to_string_lossy().into_owned()]);
    }
    Ok(command)
}

fn native(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("dylib" | "so")
        )
}

fn native_path(directory: &Path, library: &Path) -> Result<PathBuf> {
    let library = crate::config::resolve_path(directory, library);
    let resolved = if library.extension().is_some() {
        library
    } else {
        let filename = library
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| parameter("原生库名称需要UTF-8"))?;
        library.with_file_name(format!(
            "lib{filename}.{}",
            if cfg!(target_os = "macos") {
                "dylib"
            } else {
                "so"
            }
        ))
    };
    resolved.canonicalize().map_err(io_error)
}
