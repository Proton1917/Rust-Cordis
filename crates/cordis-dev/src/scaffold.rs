//! DOC-15 §12.1：项目和插件模板共用 SDK 来源与实例配置，写入采用显式创建语义。

use crate::Result;
use std::{fs, io::Write, path::Path};
use toml::{Table, Value};

const REPOSITORY: &str = "https://github.com/Proton1917/Rust-Cordis.git";
const SDK_REVISION: &str = env!("CORDIS_SOURCE_REVISION");
const PLUGIN: &str = include_str!("../templates/plugin.rs");
const WATCH: &str = include_str!("../../../plugins/watch/src/lib.rs");

/// DOC-15 §12.1：目标目录由本次命令创建，固定 SDK 来源供全部插件共用。
pub fn project(path: &Path, sdk_path: Option<&Path>) -> Result<()> {
    let sdk = sdk_dependency(sdk_path)?;
    let workspace = workspace(sdk)?;
    prepare_directory(path)?;
    for directory in ["interfaces", "contracts", "plugins"] {
        fs::create_dir(path.join(directory))?;
    }
    write_new(
        &path.join("Cargo.toml"),
        &toml::to_string_pretty(&workspace)?,
    )?;
    write_new(&path.join(".gitignore"), "/target/\n/.cordis/\n/state/\n")?;
    write_new(&path.join("cordis.toml"), CONFIGURATION)?;
    write_crate(path, "watch", WATCH, true)?;
    plugin(path, "app")?;
    write_new(&path.join("README.md"), README)?;
    Ok(())
}

/// UPSTREAM-EXT create.prepare、ADR-0032：使用新目录或已有空目录，生成失败保留当前文件。
fn prepare_directory(path: &Path) -> Result<()> {
    match fs::read_dir(path) {
        Ok(mut entries) => {
            if entries.next().transpose()?.is_none() {
                Ok(())
            } else {
                Err(format!("目标目录已有文件：{}", path.display()).into())
            }
        }
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            Ok(())
        }
        Err(problem) => Err(problem.into()),
    }
}

/// DOC-15 §12.1：解析现有项目后追加新实例，原有配置文本保留在原位置。
pub fn plugin(project: &Path, name: &str) -> Result<()> {
    validate_name(name)?;
    let workspace: Table = toml::from_str(&fs::read_to_string(project.join("Cargo.toml"))?)?;
    if workspace.get("workspace").is_none() {
        return Err("项目需要 Cargo workspace".into());
    }
    let config_path = project.join("cordis.toml");
    let config: Table = toml::from_str(&fs::read_to_string(&config_path)?)?;
    let instances = config
        .get("instance")
        .map(|instances| instances.as_table().ok_or("instance 需要 TOML 表"))
        .transpose()?;
    if instances.is_some_and(|instances| instances.contains_key(name)) {
        return Err(format!("实例 {name} 已存在").into());
    }
    let mut instance = Table::new();
    instance.insert("plugin".into(), Value::String(library(name)));
    instance.insert("component".into(), Value::String(name.into()));
    let mut instances = Table::new();
    instances.insert(name.into(), Value::Table(instance));
    let mut addition = Table::new();
    addition.insert("instance".into(), Value::Table(instances));
    let serialized = format!("\n{}", toml::to_string_pretty(&addition)?);
    write_crate(project, name, &PLUGIN.replace("{{name}}", name), false)?;
    fs::OpenOptions::new()
        .append(true)
        .open(config_path)?
        .write_all(serialized.as_bytes())?;
    Ok(())
}

/// DOC-15 §12.1：生成 cdylib manifest，watch 从项目内共用的真实实现编译。
fn write_crate(project: &Path, name: &str, source: &str, watch: bool) -> Result<()> {
    let directory = project.join("plugins").join(name);
    fs::create_dir(&directory)?;
    fs::create_dir(directory.join("src"))?;
    let mut manifest: Table = toml::from_str(PLUGIN_MANIFEST)?;
    manifest["package"]["name"] = Value::String(name.into());
    if watch {
        let mut features = Table::new();
        features.insert("rlib".into(), Value::Array(Vec::new()));
        manifest.insert("features".into(), Value::Table(features));
        let dependencies = manifest["dependencies"].as_table_mut().expect("模板依赖表");
        dependencies.insert("notify".into(), Value::String("=8.2.0".into()));
        let mut uuid = Table::new();
        uuid.insert("version".into(), Value::String("=1.26.1".into()));
        uuid.insert(
            "features".into(),
            Value::Array(vec![Value::String("v4".into())]),
        );
        dependencies.insert("uuid".into(), Value::Table(uuid));
        let mut serde = Table::new();
        serde.insert("version".into(), Value::String("=1.0.229".into()));
        serde.insert(
            "features".into(),
            Value::Array(vec![Value::String("derive".into())]),
        );
        dependencies.insert("serde".into(), Value::Table(serde));
    }
    write_new(
        &directory.join("Cargo.toml"),
        &toml::to_string_pretty(&manifest)?,
    )?;
    write_new(&directory.join("src/lib.rs"), source)
}

fn sdk_dependency(path: Option<&Path>) -> Result<Value> {
    let mut dependency = Table::new();
    match path {
        Some(path) => {
            dependency.insert(
                "path".into(),
                Value::String(path.canonicalize()?.to_string_lossy().into_owned()),
            );
        }
        None => {
            dependency.insert("git".into(), Value::String(REPOSITORY.into()));
            dependency.insert("rev".into(), Value::String(SDK_REVISION.into()));
        }
    }
    Ok(Value::Table(dependency))
}

fn workspace(sdk: Value) -> Result<Table> {
    let mut workspace: Table = toml::from_str(
        "[workspace]\nmembers = [\"plugins/*\"]\nresolver = \"3\"\n[workspace.dependencies]\n",
    )?;
    workspace["workspace"]["dependencies"]
        .as_table_mut()
        .expect("工作区依赖表")
        .insert("cordis-sdk".into(), sdk);
    Ok(workspace)
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name.as_bytes()[0].is_ascii_alphabetic()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("插件名称以字母开头，使用英文字母、数字、短横线或下划线".into());
    }
    Ok(())
}

fn library(name: &str) -> String {
    format!(
        "{}{}{}",
        std::env::consts::DLL_PREFIX,
        name.replace('-', "_"),
        std::env::consts::DLL_SUFFIX
    )
}

/// DOC-15 §12.1：每个生成文件通过 create_new 取得写入权，已有文件保持其当前内容。
fn write_new(path: &Path, content: &str) -> Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(content.as_bytes())?;
    Ok(())
}

const PLUGIN_MANIFEST: &str = r#"[package]
name = "plugin"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
publish = false
license = "MIT"

[lib]
crate-type = ["cdylib"]

[dependencies]
cordis-sdk.workspace = true
"#;

const CONFIGURATION: &str = r#"requires_core = ">=0.1.0,<0.2.0"

[runtime]
plugin_dir = "target/debug"

[instance.watch]
plugin = "watch"
component = "watch"
grants = { "cordis.manage@1" = ["install", "replace_batch", "reload"] }
config = { paths = ["target/debug"], debounce_ms = 300 }
"#;

const README: &str = r#"# Cordis 插件项目

运行 `cargo build` 编译插件，然后运行 `cordis run`。`app` 激活时发送日志事件，`watch` 监听构建产物和 `cordis.toml`，通过管理接口请求热替换和配置重读。运行状态可通过 `cordis status` 查看；`console-log` 插件把日志事件写入终端。

运行 `cordis new-plugin hello --rust` 新建插件。修改插件后再次运行 `cargo build`；正在运行的系统会装载新构建。实例配置位于 `cordis.toml`，运行状态可通过 `cordis status` 查看。

`interfaces/` 保存接口定义，`contracts/` 保存契约套件。`state/` 保存状态与检查点，`.cordis/` 保存插件版本副本。
"#;
