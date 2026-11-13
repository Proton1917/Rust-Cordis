//! SW-2、ADR-0036：签名字节变化触发真实契约，移动只更新保留副本的位置。

use super::Native;
use crate::{
    Result,
    pack::{Plan, artifacts::Artifacts, string},
    tooling,
};
use cordis_types::VerificationReceipt;
use cordis_up::payload;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use toml::{Table, Value};

/// DOC-11 §8：各库的套件结果来自已验证相同字节或本次实际运行。
pub async fn run(
    plan: &Plan,
    artifacts: &Artifacts,
    deployment: &Path,
    libraries: &[Native],
) -> Result<()> {
    let required = plan
        .configuration
        .get("runtime")
        .and_then(Value::as_table)
        .and_then(|runtime| runtime.get("require_verified"))
        .map(|value| value.as_bool().ok_or("require_verified 需要布尔值"))
        .transpose()?
        .unwrap_or(false);
    if !required {
        return Ok(());
    }
    let directory = deployment.join(".cordis/verified");
    fs::create_dir_all(&directory)?;
    for native in libraries {
        let previous = receipt(&native.source)?;
        if let Some(previous) = previous
            && retain(&previous, plan, native, &directory)?
        {
            continue;
        }
        execute(plan, artifacts, deployment, native).await?;
        let mut verified = receipt(&native.deployed)?.ok_or("真实验证结束后需要库旁印记")?;
        verified.verified_copy = verified.verified_copy.strip_prefix(&directory)?.to_owned();
        fs::write(
            path(&native.deployed),
            serde_json::to_vec_pretty(&verified)?,
        )?;
    }
    let sessions = deployment.join("target");
    match fs::remove_dir_all(sessions) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn path(source: &Path) -> PathBuf {
    let mut path = source.as_os_str().to_owned();
    path.push(".verified.json");
    PathBuf::from(path)
}

fn receipt(source: &Path) -> Result<Option<VerificationReceipt>> {
    match fs::read(path(source)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn retain(
    previous: &VerificationReceipt,
    plan: &Plan,
    native: &Native,
    directory: &Path,
) -> Result<bool> {
    let copy = if previous.verified_copy.is_absolute() {
        previous.verified_copy.clone()
    } else {
        plan.project
            .join(".cordis/verified")
            .join(&previous.verified_copy)
    };
    if fs::read(&copy)? != fs::read(&native.deployed)? {
        return Ok(false);
    }
    let name = format!(
        "{}-{}",
        native
            .deployed
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy(),
        native.deployed.file_name().unwrap().to_string_lossy()
    );
    fs::copy(&copy, directory.join(&name))?;
    let mut published = previous.clone();
    published.verified_copy = PathBuf::from(name);
    fs::write(
        path(&native.deployed),
        serde_json::to_vec_pretty(&published)?,
    )?;
    Ok(true)
}

/// ADR-0036：套件先按实际 Contract 注入与接口覆盖选择，再通过独立核心执行。
async fn execute(
    plan: &Plan,
    artifacts: &Artifacts,
    deployment: &Path,
    native: &Native,
) -> Result<()> {
    let mut suites = Vec::new();
    for library in artifacts.libraries.values() {
        let descriptor =
            tooling::describe(&plan.host, library, &plan.project, Duration::from_secs(30)).await?;
        for component in descriptor.components {
            if component
                .inject
                .iter()
                .any(|dependency| dependency.interface.starts_with("cordis.contract@"))
            {
                suites.push((library.clone(), component));
            }
        }
    }
    for component in &native.descriptor.components {
        let applicable: Vec<_> = suites
            .iter()
            .filter(|(_, suite)| covered(component, suite))
            .collect();
        if applicable.is_empty() {
            return Err(format!(
                "发布 {} 的组件 {} 需要可执行契约套件",
                native.descriptor.name, component.name
            )
            .into());
        }
        let configuration = configuration(native, &component.name)?;
        let path = deployment.join("target").join(format!(
            "{}-{}.toml",
            native.descriptor.name, component.name
        ));
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, toml::to_string_pretty(&configuration)?)?;
        for (suite, entry) in applicable {
            verify(
                plan,
                deployment,
                native,
                &component.name,
                suite,
                &entry.name,
                &path,
            )
            .await?;
        }
    }
    Ok(())
}

fn configuration(native: &Native, component: &str) -> Result<Table> {
    for instance in &native.instances {
        if string(instance, "component")?.is_none_or(|name| name == component) {
            return Ok(instance
                .get("config")
                .map(|value| value.as_table().ok_or("实例 config 需要 TOML 表"))
                .transpose()?
                .cloned()
                .unwrap_or_default());
        }
    }
    Ok(Table::new())
}

fn covered(component: &payload::Component, suite: &payload::Component) -> bool {
    component.provides.iter().any(|interface| {
        let major = interface.rsplit_once('.').map(|(major, _)| major);
        suite.inject.iter().any(|dependency| {
            dependency
                .interface
                .rsplit_once('.')
                .map(|(major, _)| major)
                == major
        })
    })
}

async fn verify(
    plan: &Plan,
    deployment: &Path,
    native: &Native,
    component: &str,
    suite: &Path,
    suite_component: &str,
    configuration: &Path,
) -> Result<()> {
    let output = tooling::command(
        vec![
            tooling::executable("cordis-dev", plan.core.parent())?
                .to_string_lossy()
                .into_owned(),
            "verify".into(),
            "--project".into(),
            deployment.to_string_lossy().into_owned(),
            "--plugin".into(),
            native.deployed.to_string_lossy().into_owned(),
            "--suite".into(),
            suite.to_string_lossy().into_owned(),
            "--component".into(),
            component.into(),
            "--suite-component".into(),
            suite_component.into(),
            "--config".into(),
            configuration.to_string_lossy().into_owned(),
        ],
        deployment,
        Duration::from_secs(60),
    )
    .await?;
    print!("{}", String::from_utf8_lossy(&output));
    Ok(())
}
