//! DOC-11 §8、ADR-0017：固定被测库副本，取得描述符，再启动两阶段契约验证部署。

use crate::{Result, receipts};
use clap::Args;
use cordis_types::ContractReport;
use cordis_up::{Limits, payload};
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use toml::{Table, Value};

#[derive(Args)]
pub struct Options {
    #[arg(long)]
    plugin: PathBuf,
    #[arg(long)]
    suite: PathBuf,
    /// 被测组件的 TOML 配置文件。
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    component: Option<String>,
    #[arg(long)]
    suite_component: Option<String>,
    #[arg(long, default_value = ".")]
    project: PathBuf,
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
}

struct Deployment {
    directory: PathBuf,
    plugin: PathBuf,
    subject: payload::Plugin,
    contract: payload::Plugin,
    interfaces: Vec<String>,
}

/// DOC-11 §8.2、RM-5：独立核心返回最终断言后保存验证副本，失败保留会话目录与诊断。
pub async fn run(options: Options) -> Result<()> {
    if options.timeout_ms == 0 {
        return Err("验证截止时间需要正整数".into());
    }
    let project = options.project.canonicalize()?;
    let source = options.plugin.canonicalize()?;
    let started = Instant::now();
    let deployment = prepare(&project, &source, &options).await?;
    let report = execute(&deployment, options.timeout_ms).await?;
    for assertion in &report.assertions {
        println!(
            "{} {} {}",
            if assertion.passed { "PASS" } else { "FAIL" },
            assertion.name,
            assertion.message
        );
    }
    let elapsed = started.elapsed();
    println!(
        "RM-5：{} ms，断言 {} 条",
        elapsed.as_millis(),
        report.assertions.len()
    );
    if !report.finished
        || !report.passed
        || report.assertions.is_empty()
        || report.assertions.iter().any(|assertion| !assertion.passed)
        || report.error.is_some()
    {
        return Err(format!(
            "契约验证失败：{}；会话 {}",
            report.error.as_deref().unwrap_or("套件断言结果未通过"),
            deployment.directory.display()
        )
        .into());
    }
    let suite = receipts::suite(
        &deployment.contract,
        deployment.interfaces,
        report.assertions,
        elapsed,
    )?;
    let receipt = receipts::save(
        &project,
        &source,
        &deployment.plugin,
        &deployment.subject.name,
        suite,
    )?;
    println!("已保存验证印记 {}", receipt.display());
    Ok(())
}

/// SW-1、DOC-11 §8：本次描述和运行使用相同库副本，原路径后续变化由验证门逐字节判定。
async fn prepare(project: &Path, source: &Path, options: &Options) -> Result<Deployment> {
    let directory = receipts::session(&project.join("target/verify"))?;
    let plugins = directory.join("plugins");
    fs::create_dir(&plugins)?;
    let plugin = plugins.join(format!("subject{}", env::consts::DLL_SUFFIX));
    let suite = plugins.join(format!("suite{}", env::consts::DLL_SUFFIX));
    fs::copy(source, &plugin)?;
    fs::copy(options.suite.canonicalize()?, &suite)?;
    let subject = describe(&plugin, &directory, options.timeout_ms).await?;
    let contract = describe(&suite, &directory, options.timeout_ms).await?;
    let component = select(&subject, options.component.as_deref())?;
    let suite_component = select(&contract, options.suite_component.as_deref())?;
    let interfaces = covered(component, suite_component)?;
    let configuration = match &options.config {
        Some(path) => toml::from_str(std::str::from_utf8(&receipts::read(path)?)?)?,
        None => Table::new(),
    };
    write_configuration(
        &directory,
        &plugin,
        &suite,
        component,
        suite_component,
        configuration,
    )?;
    Ok(Deployment {
        directory,
        plugin,
        subject,
        contract,
        interfaces,
    })
}

/// Q0、CP-4/CP-7：描述符由 cordis-host 子进程读取，开发工具保持纯数据调用路径。
async fn describe(path: &Path, project: &Path, timeout: u64) -> Result<payload::Plugin> {
    let command = vec![
        executable("cordis-host")?.to_string_lossy().into_owned(),
        "--describe".into(),
        path.to_string_lossy().into_owned(),
        "--cache-dir".into(),
        project
            .join(".cordis/lib/discovery")
            .to_string_lossy()
            .into_owned(),
    ];
    let output = cordis_process::run_capture(
        &command,
        project,
        Duration::from_millis(timeout),
        Limits::DEFAULT.max_frame,
    )
    .await?;
    Ok(serde_json::from_slice(&output)?)
}

fn select<'a>(plugin: &'a payload::Plugin, name: Option<&str>) -> Result<&'a payload::Component> {
    match name {
        Some(name) => plugin
            .components
            .iter()
            .find(|component| component.name == name)
            .ok_or_else(|| format!("插件 {} 的描述符缺少组件 {name}", plugin.name).into()),
        None if plugin.components.len() == 1 => Ok(&plugin.components[0]),
        None => Err(format!(
            "插件 {} 有 {} 个组件，请用组件选项指定被测对象",
            plugin.name,
            plugin.components.len()
        )
        .into()),
    }
}

/// DOC-11 §3/§8.3：套件声明的接口与被测提供声明相交，印记记录覆盖的接口主版本。
fn covered(subject: &payload::Component, suite: &payload::Component) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for provided in &subject.provides {
        let major = interface_major(provided)?;
        if suite
            .inject
            .iter()
            .any(|dependency| interface_major(&dependency.interface).is_ok_and(|key| key == major))
        {
            result.push(major);
        }
    }
    result.sort();
    result.dedup();
    if result.is_empty() {
        return Err("套件注入接口与被测组件提供接口缺少交集".into());
    }
    Ok(result)
}

fn interface_major(interface: &str) -> Result<String> {
    let (name, version) = interface.rsplit_once('@').ok_or("接口声明需要 @ 版本")?;
    let (major, minor) = version
        .split_once('.')
        .ok_or("接口版本需要主版本与次版本")?;
    let major: u32 = major.parse()?;
    let _: u32 = minor.parse()?;
    Ok(format!("{name}@{major}"))
}

/// ADR-0017：被测组件和套件分别成组，由核心管理依赖与两阶段迁移。
fn write_configuration(
    directory: &Path,
    plugin: &Path,
    suite: &Path,
    component: &payload::Component,
    contract: &payload::Component,
    config: Table,
) -> Result<()> {
    let mut instances = Table::new();
    for (name, path, component, config) in [
        ("subject", plugin, component, config),
        ("suite", suite, contract, Table::new()),
    ] {
        let mut instance = Table::new();
        instance.insert(
            "plugin".into(),
            Value::String(path.file_name().unwrap().to_string_lossy().into_owned()),
        );
        instance.insert("component".into(), Value::String(component.name.clone()));
        instance.insert("group".into(), Value::String(name.into()));
        instance.insert("config".into(), Value::Table(config));
        instances.insert(name.into(), Value::Table(instance));
    }
    let mut settings: Table = toml::from_str(
        "requires_core = \">=0.1.0,<0.2.0\"\n[runtime]\nconfirm_ms = 0\nrestart_max = 0\ncheckpoint_ms = 0\n",
    )?;
    settings.insert("instance".into(), Value::Table(instances));
    fs::write(
        directory.join("cordis.toml"),
        toml::to_string_pretty(&settings)?,
    )?;
    Ok(())
}

/// DOC-15 §3.1：开发工具启动独立核心，报告文件保留阶段失败，进程错误保持原诊断。
async fn execute(deployment: &Deployment, timeout: u64) -> Result<ContractReport> {
    let report = deployment.directory.join("contract-report.json");
    let command = vec![
        executable("cordis")?.to_string_lossy().into_owned(),
        "run".into(),
        "--contract-suite".into(),
        "suite".into(),
        "--contract-subject".into(),
        "subject".into(),
        "--contract-report".into(),
        report.to_string_lossy().into_owned(),
        "--contract-timeout-ms".into(),
        timeout.to_string(),
    ];
    let executed = cordis_process::run_capture(
        &command,
        &deployment.directory,
        Duration::from_millis(timeout).saturating_add(Duration::from_secs(30)),
        Limits::DEFAULT.max_frame,
    )
    .await;
    let result: ContractReport = match fs::metadata(&report) {
        Ok(_) => serde_json::from_slice(&receipts::read(&report)?)?,
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
            executed?;
            return Err("验证核心退出时缺少契约报告".into());
        }
        Err(problem) => return Err(problem.into()),
    };
    if let Err(problem) = executed {
        if result.finished && result.passed {
            return Err(problem.into());
        }
        eprintln!("验证核心：{problem}");
    }
    Ok(result)
}

fn executable(name: &str) -> Result<PathBuf> {
    let current = env::current_exe()?;
    let sibling = current.parent().expect("可执行文件父目录").join(name);
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
    Err(format!("程序 {name} 安装路径缺席").into())
}
