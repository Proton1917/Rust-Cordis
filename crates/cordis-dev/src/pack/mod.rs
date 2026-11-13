//! DOC-15 §12.2、ADR-0036：构建、装配、签名与归档组成一次发布。

mod artifacts;
mod files;
mod platform;
mod plugins;
mod signing;

use crate::{Result, tooling};
use artifacts::Artifacts;
use clap::Args;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use toml::{Table, Value};

#[derive(Args)]
pub struct Options {
    #[arg(long, default_value = ".")]
    project: PathBuf,
    #[arg(long, default_value = "dist")]
    out: PathBuf,
    #[arg(long)]
    target: String,
    #[arg(long)]
    app: bool,
    #[arg(long)]
    sign: Option<String>,
    #[arg(long, requires = "sign")]
    allow_third_party: bool,
    #[arg(long)]
    runtime_dir: Option<PathBuf>,
}

struct Plan {
    project: PathBuf,
    configuration: Table,
    settings: Table,
    name: String,
    version: String,
    identifier: String,
    core: PathBuf,
    host: PathBuf,
    platform: platform::Platform,
}

/// AU-12：输出目录由本次命令创建；失败保留装配目录与具体错误。
pub async fn run(options: Options) -> Result<()> {
    let mut plan = plan(&options).await?;
    let artifacts = artifacts::build(&plan.project, &options.target).await?;
    let out = absolute(&plan.project, &options.out);
    fs::create_dir_all(&out)?;
    let staging = plan.project.join("target/pack");
    fs::create_dir_all(&staging)?;
    let destination = if options.app {
        out.join(format!("{}.app", plan.name))
    } else {
        staging.join(format!("{}-{}-{}", plan.name, plan.version, options.target))
    };
    fs::create_dir(&destination)?;
    let (binaries, deployment) = layout(&destination, options.app)?;
    fs::copy(&plan.core, binaries.join("cordis"))?;
    fs::copy(&plan.host, binaries.join("cordis-host"))?;
    let native = plugins::assemble(
        &mut plan,
        &artifacts,
        &deployment,
        options.app.then_some(binaries.as_path()),
    )
    .await?;
    assets(&plan, &deployment)?;
    configuration(&mut plan, &deployment, options.app)?;
    if options.app {
        platform::bundle(&plan, &destination)?;
    }
    let code = platform::code_files(&destination, plan.platform)?;
    signing::sign(&plan, &options, &destination, &binaries, &code).await?;
    plugins::verification(&plan, &artifacts, &deployment, &native).await?;
    service(&plan, &binaries, &deployment)?;
    signing::finish(&plan, &options, &destination, &code).await?;
    let delivered = if options.app {
        destination
    } else {
        files::archive(&destination, &out)?
    };
    println!("已生成发布包 {}", delivered.display());
    Ok(())
}

/// ADR-0036：编译与装载都在目标平台执行，版本检查使用发布核心的实际版本。
async fn plan(options: &Options) -> Result<Plan> {
    let platform = platform::Platform::parse(&options.target)?;
    platform.local()?;
    if options.app && !platform.macos() {
        return Err("app 发布格式需要 macOS 目标".into());
    }
    let project = options.project.canonicalize()?;
    let configuration: Table = toml::from_str(&fs::read_to_string(project.join("cordis.toml"))?)?;
    let settings = configuration
        .get("pack")
        .map(|value| value.as_table().ok_or("pack 需要 TOML 表"))
        .transpose()?
        .cloned()
        .unwrap_or_default();
    let name = string(&settings, "name")?
        .map(str::to_owned)
        .unwrap_or_else(|| project.file_name().unwrap().to_string_lossy().into_owned());
    if name.is_empty() || Path::new(&name).components().count() != 1 {
        return Err("发布名称需要单个目录名称".into());
    }
    let version = string(&settings, "version")?.unwrap_or("0.1.0").to_owned();
    semver::Version::parse(&version)?;
    let identifier = string(&settings, "identifier")?
        .map(str::to_owned)
        .unwrap_or_else(|| format!("org.cordis.{name}"));
    if identifier.is_empty()
        || !identifier
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'.' | b'-'))
    {
        return Err("pack.identifier 使用英文字母、数字、点和短横线".into());
    }
    let core = tooling::executable("cordis", options.runtime_dir.as_deref())?;
    let host = tooling::executable("cordis-host", options.runtime_dir.as_deref())?;
    platform.binary(&core)?;
    platform.binary(&host)?;
    core_version(&core, &project, &configuration).await?;
    Ok(Plan {
        project,
        configuration,
        settings,
        name,
        version,
        identifier,
        core,
        host,
        platform,
    })
}

/// DOC-15 §12.2：核心的实际版本满足部署声明后进入产物构建。
async fn core_version(core: &Path, project: &Path, configuration: &Table) -> Result<()> {
    let output = tooling::command(
        vec![core.to_string_lossy().into_owned(), "version".into()],
        project,
        Duration::from_secs(10),
    )
    .await?;
    let output = std::str::from_utf8(&output)?.trim();
    let core_version = semver::Version::parse(
        output
            .strip_prefix("cordis ")
            .ok_or("核心版本输出需要 cordis 前缀")?,
    )?;
    let required = string(configuration, "requires_core")?.unwrap_or("*");
    if !semver::VersionReq::parse(required)?.matches(&core_version) {
        return Err(format!("部署需要核心 {required}，打包核心版本为 {core_version}").into());
    }
    Ok(())
}

/// DOC-15 §12.2：可执行程序与配置分别放在平台约定目录。
fn layout(destination: &Path, app: bool) -> Result<(PathBuf, PathBuf)> {
    let (binaries, deployment) = if app {
        (
            destination.join("Contents/MacOS"),
            destination.join("Contents/Resources"),
        )
    } else {
        (destination.to_owned(), destination.to_owned())
    };
    fs::create_dir_all(&binaries)?;
    fs::create_dir_all(deployment.join("plugins"))?;
    Ok((binaries, deployment))
}

fn configuration(plan: &mut Plan, deployment: &Path, app: bool) -> Result<()> {
    let runtime = plan
        .configuration
        .entry("runtime")
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or("runtime 需要 TOML 表")?;
    runtime.insert("plugin_dir".into(), Value::String("plugins".into()));
    let state = if app {
        string(&plan.settings, "state_dir")?
            .unwrap_or("state")
            .to_owned()
    } else {
        "state".into()
    };
    runtime.insert("state_dir".into(), Value::String(state));
    let settings = plan
        .configuration
        .entry("pack")
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .expect("已解析 pack 表");
    settings.insert("app".into(), Value::Boolean(app));
    settings.insert("identifier".into(), Value::String(plan.identifier.clone()));
    if let Some(paths) = settings
        .get_mut("runtime_paths")
        .and_then(Value::as_table_mut)
    {
        for (name, source) in paths.iter_mut() {
            *source = Value::String(format!("runtimes/{name}"));
        }
    }
    fs::write(
        deployment.join("cordis.toml"),
        toml::to_string_pretty(&plan.configuration)?,
    )?;
    Ok(())
}

/// DOC-15 §12.2：显式资源列表随部署复制，供 HTTP 等插件的配置路径引用。
fn assets(plan: &Plan, deployment: &Path) -> Result<()> {
    let selected = plan
        .settings
        .get("assets")
        .map(|value| value.as_array().ok_or("pack.assets 需要数组"))
        .transpose()?;
    for value in selected.into_iter().flatten() {
        let name = value.as_str().ok_or("资源路径需要字符串")?;
        let path = Path::new(name);
        let reserved = [
            "plugins",
            "runtimes",
            "service",
            ".cordis",
            "state",
            "cordis",
            "cordis-host",
            "cordis.toml",
        ];
        if path.is_absolute()
            || path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
            || path
                .components()
                .next()
                .is_some_and(|part| reserved.iter().any(|name| part.as_os_str() == *name))
        {
            return Err("发布资源使用独立于运行目录的相对路径".into());
        }
        let source = plan.project.join(path);
        let destination = deployment.join(path);
        fs::create_dir_all(destination.parent().ok_or("资源需要目录路径")?)?;
        let metadata = fs::metadata(&source)?;
        if metadata.is_dir() {
            fs::create_dir(&destination)?;
            files::copy_tree(&source, &destination)?;
        } else {
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)?;
            std::io::copy(&mut fs::File::open(source)?, &mut output)?;
            fs::set_permissions(destination, metadata.permissions())?;
        }
    }
    Ok(())
}

fn service(plan: &Plan, binaries: &Path, deployment: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let directory = deployment.join("service");
    fs::create_dir(&directory)?;
    let core = pathdiff::diff_paths(binaries.join("cordis"), deployment)
        .ok_or("服务与核心需要共同发布根目录")?;
    let installer = directory.join("install.sh");
    fs::write(
        &installer,
        format!(
            "#!/bin/sh\nset -eu\nproject=$(CDPATH= cd -- \"$(dirname -- \"$0\")/..\" && pwd)\nexec \"$project/{}\" service install --name '{}' --config \"$project/cordis.toml\" \"$@\"\n",
            core.display(),
            plan.identifier
        ),
    )?;
    fs::set_permissions(&installer, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn absolute(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}

fn string<'a>(table: &'a Table, name: &str) -> Result<Option<&'a str>> {
    table
        .get(name)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("{name} 需要字符串").into())
        })
        .transpose()
}
