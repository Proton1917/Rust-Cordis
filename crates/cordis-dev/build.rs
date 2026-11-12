//! ADR-0032：生成项目使用开发工具构建时所在的既有 Git 修订。

use std::{env, process::Command};

fn git(arguments: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("git").args(arguments).output()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=CORDIS_SOURCE_REVISION");
    let revision = match env::var("CORDIS_SOURCE_REVISION") {
        Ok(revision) => revision,
        Err(env::VarError::NotPresent) => {
            let head = git(&["rev-parse", "HEAD"])?;
            if !head.status.success() {
                return Err("构建开发工具需要 Git 来源或 CORDIS_SOURCE_REVISION".into());
            }
            watch_git("HEAD")?;
            let branch = git(&["symbolic-ref", "--quiet", "HEAD"])?;
            if branch.status.success() {
                watch_git(String::from_utf8(branch.stdout)?.trim())?;
            }
            String::from_utf8(head.stdout)?.trim().to_owned()
        }
        Err(problem) => return Err(problem.into()),
    };
    if !matches!(revision.len(), 40 | 64) || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("SDK 来源需要完整 Git 提交编号".into());
    }
    println!("cargo:rustc-env=CORDIS_SOURCE_REVISION={revision}");
    Ok(())
}

fn watch_git(reference: &str) -> Result<(), Box<dyn std::error::Error>> {
    let location = git(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        reference,
    ])?;
    if !location.status.success() {
        return Err("读取 Git 引用位置失败".into());
    }
    println!(
        "cargo:rerun-if-changed={}",
        String::from_utf8(location.stdout)?.trim()
    );
    Ok(())
}
