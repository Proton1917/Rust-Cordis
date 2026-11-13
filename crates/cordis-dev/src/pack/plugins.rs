//! DOC-15 §12.2、ADR-0036：插件引用改写为发布目录，验证结果继续绑定实际库字节。

mod verification;

use super::{Artifacts, Plan, absolute, artifacts, files, string};
use crate::Result;
use cordis_up::payload;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use toml::{Table, Value};

pub struct Native {
    pub source: PathBuf,
    pub deployed: PathBuf,
    pub descriptor: payload::Plugin,
    pub instances: Vec<Table>,
}

/// AU-12：同一引用复制一次，各实例保留组件、配置与隔离元数据。
pub async fn assemble(
    plan: &mut Plan,
    artifacts: &Artifacts,
    deployment: &Path,
    runtime_root: &BTreeMap<String, PathBuf>,
    app_binaries: Option<&Path>,
) -> Result<Vec<Native>> {
    let requested = requested(&plan.configuration)?;
    let plugin_dir = plan
        .configuration
        .get("runtime")
        .and_then(Value::as_table)
        .map(|runtime| string(runtime, "plugin_dir"))
        .transpose()?
        .flatten()
        .unwrap_or("plugins");
    let plugin_dir = absolute(&plan.project, Path::new(plugin_dir));
    let mut mapping = BTreeMap::new();
    let mut native = Vec::new();
    for (index, (reference, instances)) in requested.into_iter().enumerate() {
        let original = absolute(&plugin_dir, Path::new(&reference));
        let target = format!("plugin-{}", index + 1);
        let root = deployment.join("plugins").join(&target);
        fs::create_dir(&root)?;
        let package = if original.is_dir() {
            directory(
                plan,
                artifacts,
                &original,
                &root,
                runtime_root,
                app_binaries,
            )
            .await?
        } else {
            let name = artifacts::filename(&original)?;
            let filename = if original.extension().is_some() {
                name
            } else {
                plan.platform.library(&name)
            };
            let source = artifacts
                .libraries
                .get(&filename)
                .cloned()
                .unwrap_or(original);
            native_package(plan, source, &root, None).await?
        };
        if let Some(mut package) = package {
            package.instances = instances;
            native.push(package);
        }
        mapping.insert(reference, target);
    }
    references(&mut plan.configuration, &mapping);
    Ok(native)
}

/// DOC-15 §12.2：按部署引用合并同一个库的组件配置。
pub(super) fn requested(configuration: &Table) -> Result<BTreeMap<String, Vec<Table>>> {
    let instances = configuration
        .get("instance")
        .and_then(Value::as_table)
        .ok_or("发布配置需要 instance 表")?;
    let mut requested: BTreeMap<String, Vec<Table>> = BTreeMap::new();
    for instance in instances.values() {
        let instance = instance.as_table().ok_or("实例配置需要 TOML 表")?;
        let reference = string(instance, "plugin")?.ok_or("实例需要 plugin 引用")?;
        requested
            .entry(reference.into())
            .or_default()
            .push(instance.clone());
    }
    Ok(requested)
}

/// ADR-0036：已解析实例使用本轮装配产生的完整引用映射。
fn references(configuration: &mut Table, mapping: &BTreeMap<String, String>) {
    for (_, instance) in configuration
        .get_mut("instance")
        .unwrap()
        .as_table_mut()
        .unwrap()
        .iter_mut()
    {
        let instance = instance.as_table_mut().unwrap();
        let reference = instance["plugin"].as_str().unwrap();
        instance["plugin"] = Value::String(mapping[reference].clone());
    }
}

/// DOC-15 §9：目录清单保留原元数据，原生库与进程命令按其插口分别发布。
async fn directory(
    plan: &Plan,
    artifacts: &Artifacts,
    source: &Path,
    destination: &Path,
    runtimes: &BTreeMap<String, PathBuf>,
    app_binaries: Option<&Path>,
) -> Result<Option<Native>> {
    let mut manifest: Table =
        toml::from_str(&fs::read_to_string(source.join("cordis-plugin.toml"))?)?;
    match string(&manifest, "socket")?.ok_or("插件清单需要 socket")? {
        "native" => {
            let declared = string(&manifest, "library")?.ok_or("原生清单需要 library")?;
            let original = absolute(source, Path::new(declared));
            let filename = if original.extension().is_some() {
                artifacts::filename(&original)?
            } else {
                plan.platform.library(&artifacts::filename(&original)?)
            };
            let library = artifacts
                .libraries
                .get(&filename)
                .cloned()
                .unwrap_or(original);
            assets(plan, source, destination, &manifest)?;
            native_package(plan, library, destination, Some(manifest)).await
        }
        "process" => {
            if manifest.get("protocol").and_then(Value::as_integer) != Some(1) {
                return Err("进程插件需要 UP/1".into());
            }
            if source.join("Cargo.toml").is_file() {
                assets(plan, source, destination, &manifest)?;
            } else {
                files::copy_tree(source, destination, &plan.secret_paths)?;
            }
            process_command(
                plan,
                artifacts,
                source,
                destination,
                &mut manifest,
                runtimes,
                app_binaries,
            )?;
            fs::write(
                destination.join("cordis-plugin.toml"),
                toml::to_string_pretty(&manifest)?,
            )?;
            Ok(None)
        }
        "web" => {
            if manifest.get("web").is_none() {
                return Err("网页插件需要 web 资源声明".into());
            }
            assets(plan, source, destination, &manifest)?;
            fs::write(
                destination.join("cordis-plugin.toml"),
                toml::to_string_pretty(&manifest)?,
            )?;
            Ok(None)
        }
        other => Err(format!("发布清单需要 native、process 或 web，收到 {other}").into()),
    }
}

async fn native_package(
    plan: &Plan,
    source: PathBuf,
    destination: &Path,
    declared: Option<Table>,
) -> Result<Option<Native>> {
    let descriptor = plan
        .descriptions
        .get(&source.canonicalize()?)
        .ok_or("原生发布描述符缺席")?
        .clone();
    let mut manifest = declared.unwrap_or_default();
    for (name, value) in [
        ("name", descriptor.name.as_str()),
        ("version", descriptor.version.as_str()),
    ] {
        if let Some(previous) = string(&manifest, name)?
            && previous != value
        {
            return Err(format!("原生清单 {name} 与实际描述符失配").into());
        }
        manifest.insert(name.into(), Value::String(value.into()));
    }
    manifest.insert("socket".into(), Value::String("native".into()));
    let filename = artifacts::filename(&source)?;
    let deployed = destination.join(&filename);
    fs::copy(&source, &deployed)?;
    manifest.insert("library".into(), Value::String(filename));
    fs::write(
        destination.join("cordis-plugin.toml"),
        toml::to_string_pretty(&manifest)?,
    )?;
    Ok(Some(Native {
        source,
        deployed,
        descriptor,
        instances: Vec::new(),
    }))
}

fn assets(plan: &Plan, source: &Path, destination: &Path, manifest: &Table) -> Result<()> {
    if let Some(web) = manifest.get("web").and_then(Value::as_table) {
        let assets = string(web, "assets")?.ok_or("web 需要 assets")?;
        let path = Path::new(assets);
        if path.is_absolute()
            || path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            return Err("发布 assets 需要插件目录内的相对路径".into());
        }
        files::copy_tree(
            &source.join(path),
            &destination.join(path),
            &plan.secret_paths,
        )?;
    }
    Ok(())
}

/// DOC-15 §12.2：运行时来源显式声明，解释器入口在复制时确认。
pub(super) fn runtimes(plan: &Plan, deployment: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let selected = plan
        .settings
        .get("runtimes")
        .map(|value| value.as_array().ok_or("runtimes 需要数组"))
        .transpose()?;
    let mut paths = BTreeMap::new();
    let mut requested = BTreeSet::new();
    for runtime in selected.into_iter().flatten() {
        requested.insert(runtime.as_str().ok_or("runtimes 元素需要字符串")?);
    }
    for name in requested {
        let executable = match name {
            "python" => "python3",
            "node" => "node",
            _ => return Err(format!("运行时名称支持 python、node，收到 {name}").into()),
        };
        let sources = plan
            .settings
            .get("runtime_paths")
            .and_then(Value::as_table)
            .ok_or("打包解释器需要 pack.runtime_paths")?;
        let source = string(sources, name)?.ok_or_else(|| format!("runtime_paths 缺少 {name}"))?;
        let source = absolute(&plan.project, Path::new(source));
        let destination = deployment.join("runtimes").join(name);
        files::copy_tree(&source, &destination, &plan.secret_paths)?;
        let binary = destination.join("bin").join(executable);
        plan.platform.binary(&binary)?;
        paths.insert(name.to_owned(), binary);
    }
    Ok(paths)
}

fn process_command(
    plan: &Plan,
    artifacts: &Artifacts,
    source: &Path,
    destination: &Path,
    manifest: &mut Table,
    runtimes: &BTreeMap<String, PathBuf>,
    app_binaries: Option<&Path>,
) -> Result<()> {
    let command = manifest
        .get_mut("command")
        .and_then(Value::as_array_mut)
        .ok_or("进程清单需要 command 数组")?;
    let first = command
        .first()
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("command 需要启动程序")?;
    let name = artifacts::filename(Path::new(first))?;
    let runtime = match name.as_str() {
        "python" | "python3" => runtimes.get("python"),
        "node" => runtimes.get("node"),
        _ => None,
    };
    if let Some(runtime) = runtime {
        command[0] = Value::String(
            pathdiff::diff_paths(runtime, destination)
                .ok_or("解释器与插件需要共同发布根目录")?
                .to_string_lossy()
                .into_owned(),
        );
    } else if matches!(name.as_str(), "python" | "python3" | "node") {
        return Ok(());
    } else if let Some(binary) = artifacts.binaries.get(&name) {
        plan.platform.binary(binary)?;
        let executable = window_binary(&name, destination, app_binaries);
        fs::copy(binary, &executable)?;
        command[0] = Value::String(relative_command(&executable, destination)?);
    } else {
        let binary = absolute(source, Path::new(first));
        if binary.is_file() {
            if Path::new(first).is_absolute()
                || source.join("Cargo.toml").is_file()
                || (name == "cordis-window" && app_binaries.is_some())
            {
                let executable = window_binary(&name, destination, app_binaries);
                let copied = destination.join(first);
                fs::copy(binary, &executable)?;
                let inside = !Path::new(first).is_absolute()
                    && !Path::new(first)
                        .components()
                        .any(|part| part == std::path::Component::ParentDir);
                if inside && !source.join("Cargo.toml").is_file() && executable != copied {
                    fs::remove_file(copied)?;
                }
                command[0] = Value::String(relative_command(&executable, destination)?);
            }
        } else if Path::new(first).is_absolute() {
            return Err(format!("进程程序缺席：{first}").into());
        }
    }
    Ok(())
}

fn window_binary(name: &str, destination: &Path, app_binaries: Option<&Path>) -> PathBuf {
    if name == "cordis-window" {
        app_binaries.unwrap_or(destination).join(name)
    } else {
        destination.join(name)
    }
}

fn relative_command(executable: &Path, directory: &Path) -> Result<String> {
    let relative =
        pathdiff::diff_paths(executable, directory).ok_or("进程程序与插件需要共同发布根目录")?;
    Ok(format!("./{}", relative.display()))
}

pub async fn verification(
    plan: &Plan,
    artifacts: &Artifacts,
    deployment: &Path,
    native: &[Native],
) -> Result<()> {
    verification::run(plan, artifacts, deployment, native).await
}
