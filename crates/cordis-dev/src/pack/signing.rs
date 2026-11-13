//! DOC-15 §12.2、ADR-0036：签名修改发布副本，entitlement 按 Host 的装载职责设置。

use super::{Options, Plan};
use crate::{Result, tooling};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// AU-12：先签署代码，契约检查其最终字节；bundle 外层封装在资源写入后签署。
pub async fn sign(
    plan: &Plan,
    options: &Options,
    destination: &Path,
    binaries: &Path,
    code: &[PathBuf],
) -> Result<()> {
    if !plan.platform.macos() {
        if options.sign.is_some() {
            return Err("代码签名选项需要 macOS 目标".into());
        }
        return Ok(());
    }
    if let Some(identity) = &options.sign {
        let entitlement = if options.allow_third_party {
            let path = destination.join("host.entitlements.plist");
            let mut values = plist::Dictionary::new();
            values.insert(
                "com.apple.security.cs.disable-library-validation".into(),
                plist::Value::Boolean(true),
            );
            plist::Value::Dictionary(values).to_file_xml(&path)?;
            Some(path)
        } else {
            None
        };
        let host = binaries.join("cordis-host");
        for file in code {
            sign_file(
                file,
                identity,
                if file == &host {
                    entitlement.as_deref()
                } else {
                    None
                },
                true,
                destination,
            )
            .await?;
        }
        if let Some(path) = entitlement {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
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
