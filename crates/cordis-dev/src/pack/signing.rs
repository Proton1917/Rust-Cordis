//! DOC-15 §12.2、ADR-0036/0043：签名修改发布副本，权限按Host与脚本运行时的职责设置。

use super::{Options, Plan};
use crate::{Result, tooling};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

/// AU-12：先签署代码，契约检查其最终字节；bundle 外层封装在资源写入后签署。
pub async fn sign(
    plan: &Plan,
    options: &Options,
    destination: &Path,
    binaries: &Path,
    runtimes: &BTreeMap<String, PathBuf>,
    code: &[PathBuf],
) -> Result<()> {
    if !plan.platform.macos() {
        if options.sign.is_some() {
            return Err("代码签名选项需要 macOS 目标".into());
        }
        return Ok(());
    }
    if let Some(identity) = &options.sign {
        let entitlements = permissions(options, destination, binaries, runtimes, identity)?;
        for file in code {
            sign_file(
                file,
                identity,
                entitlements.get(file).map(PathBuf::as_path),
                true,
                destination,
            )
            .await?;
        }
        for path in entitlements.values() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// ADR-0043：权限按实际解释器入口归属，Node保留JIT，ad-hoc解释器可装入同包扩展。
fn permissions(
    options: &Options,
    destination: &Path,
    binaries: &Path,
    runtimes: &BTreeMap<String, PathBuf>,
    identity: &str,
) -> Result<BTreeMap<PathBuf, PathBuf>> {
    let mut selected = BTreeMap::new();
    if options.allow_third_party {
        let path = entitlement(destination, "host", &["disable-library-validation"])?;
        selected.insert(binaries.join("cordis-host").canonicalize()?, path);
    }
    for (name, executable) in runtimes {
        let mut capabilities = Vec::new();
        if name == "node" {
            capabilities.extend(["allow-jit", "allow-unsigned-executable-memory"]);
        }
        if identity == "-" {
            capabilities.push("disable-library-validation");
        }
        if !capabilities.is_empty() {
            let path = entitlement(destination, name, &capabilities)?;
            selected.insert(executable.canonicalize()?, path);
        }
    }
    Ok(selected)
}

/// ADR-0036/0043：临时plist表达本次签名角色，成功签署后由调用方释放。
fn entitlement(destination: &Path, role: &str, capabilities: &[&str]) -> Result<PathBuf> {
    let path = destination.join(format!("{role}.entitlements.plist"));
    let values = capabilities
        .iter()
        .map(|name| {
            (
                format!("com.apple.security.cs.{name}"),
                plist::Value::Boolean(true),
            )
        })
        .collect();
    plist::Value::Dictionary(values).to_file_xml(&path)?;
    Ok(path)
}

pub async fn finish(
    plan: &Plan,
    options: &Options,
    destination: &Path,
    code: &[PathBuf],
) -> Result<()> {
    if !plan.platform.macos() {
        return Ok(());
    }
    if options.app {
        sign_file(
            destination,
            options.sign.as_deref().unwrap_or("-"),
            None,
            options.sign.is_some(),
            destination,
        )
        .await?;
        verify(destination, destination).await?;
    }
    for binary in code {
        verify(binary, destination).await?;
    }
    Ok(())
}

async fn sign_file(
    path: &Path,
    identity: &str,
    entitlement: Option<&Path>,
    hardened: bool,
    project: &Path,
) -> Result<()> {
    let mut command = vec![
        "/usr/bin/codesign".into(),
        "--force".into(),
        "--sign".into(),
        identity.into(),
    ];
    if hardened {
        command.extend(["--options".into(), "runtime".into()]);
    }
    if let Some(entitlement) = entitlement {
        command.extend([
            "--entitlements".into(),
            entitlement.to_string_lossy().into_owned(),
        ]);
    }
    command.push(path.to_string_lossy().into_owned());
    tooling::command(command, project, Duration::from_secs(60)).await?;
    Ok(())
}

async fn verify(path: &Path, project: &Path) -> Result<()> {
    tooling::command(
        vec![
            "/usr/bin/codesign".into(),
            "--verify".into(),
            "--strict".into(),
            path.to_string_lossy().into_owned(),
        ],
        project,
        Duration::from_secs(30),
    )
    .await?;
    Ok(())
}
