//! DOC-15 §12.2：对实际交付树中的二进制与清单执行路径隐私检查。

use crate::Result;
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

struct Pattern {
    label: &'static str,
    bytes: Vec<u8>,
}

/// 发布边界使用完整字节匹配，诊断只列相对文件名和命中次数。
pub(super) fn check(root: &Path, warning: bool) -> Result<()> {
    let patterns = patterns()?;
    let mut files = Vec::new();
    collect(root, &mut files)?;
    let mut findings = Vec::new();
    for file in files {
        let bytes = fs::read(&file)?;
        let counts = patterns
            .iter()
            .map(|pattern| {
                (
                    pattern.label,
                    memchr::memmem::find_iter(&bytes, &pattern.bytes).count(),
                )
            })
            .filter(|(_, count)| *count > 0)
            .map(|(label, count)| format!("{label}={count}"))
            .collect::<Vec<_>>();
        if !counts.is_empty() {
            findings.push(format!(
                "{}: {}",
                file.strip_prefix(root)?.display(),
                counts.join(", ")
            ));
        }
    }
    if findings.is_empty() {
        return Ok(());
    }
    let message = format!(
        "发布路径隐私检查命中：\n{}\n可使用官方发布包的 bin 目录作为 --runtime-dir，或使用路径映射重建的 runtime；显式 --allow-private-paths 将检查降级为警告。",
        findings.join("\n")
    );
    if warning {
        eprintln!("警告：{message}");
        Ok(())
    } else {
        Err(message.into())
    }
}

/// ADR-0049：以构建环境的 HOME 和系统当前用户确定检索对象。
fn patterns() -> Result<[Pattern; 2]> {
    let home = env::var("HOME")?;
    let username = Command::new("id").arg("-un").output()?;
    if !username.status.success() {
        return Err("读取当前用户名失败".into());
    }
    let username = String::from_utf8(username.stdout)?.trim().to_owned();
    if home.is_empty() || username.is_empty() {
        return Err("路径隐私检查需要 HOME 与用户名".into());
    }
    Ok([
        Pattern {
            label: "HOME",
            bytes: home.into_bytes(),
        },
        Pattern {
            label: "username",
            bytes: username.into_bytes(),
        },
    ])
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(&entry.path(), files)?;
        } else if kind.is_file() {
            let path = entry.path();
            let manifest = matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("toml" | "json" | "plist")
            );
            let mut prefix = Vec::with_capacity(16);
            fs::File::open(&path)?.take(16).read_to_end(&mut prefix)?;
            if manifest
                || matches!(
                    object::FileKind::parse(prefix.as_slice()),
                    Ok(object::FileKind::MachO32
                        | object::FileKind::MachO64
                        | object::FileKind::MachOFat32
                        | object::FileKind::MachOFat64
                        | object::FileKind::Elf32
                        | object::FileKind::Elf64)
                )
            {
                files.push(path);
            }
        }
    }
    Ok(())
}
