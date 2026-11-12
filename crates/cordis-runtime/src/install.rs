//! DOC-15 §3.2：以序号复制插件来源，按完整字节比较复用已安装副本。

use crate::{
    config::{Result, parameter},
    storage::io_error,
};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

/// DOC-15 §3.2：来源位于授权目录时建立不可变引用，复制失败清理本次暂存文件。
pub fn install(source: &Path, project: &Path, data: &Path, plugins: &Path) -> Result<String> {
    let source = source.canonicalize().map_err(io_error)?;
    if !source.starts_with(project) && !source.starts_with(data) {
        return Err(cordis_up::WireError::new(
            9,
            "安装来源需要位于项目或调用方数据目录",
        ));
    }
    if source.is_dir() && plugins.starts_with(&source) {
        return Err(parameter("安装目录需要与插件存储目录分离"));
    }
    let (existing, sequence) = installed_reference(&source, plugins)?;
    if let Some(reference) = existing {
        copy_receipt(&source, &plugins.join(&reference))?;
        return Ok(reference);
    }
    let next = sequence
        .checked_add(1)
        .ok_or_else(|| parameter("插件安装序号已耗尽"))?;
    let suffix = if source.is_file() {
        source
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let reference = format!("installed-{next}{suffix}");
    publish_copy(
        &source,
        &plugins.join(format!(".install-{next}")),
        &plugins.join(&reference),
    )?;
    copy_receipt(&source, &plugins.join(&reference))?;
    Ok(reference)
}

/// DOC-11 SW-2：单库安装同步旁置验证印记，宿主继续核验保留副本的完整字节与接口覆盖。
fn copy_receipt(source: &Path, destination: &Path) -> Result<()> {
    if source.is_dir() {
        return Ok(());
    }
    let receipt_path = |path: &Path| {
        let mut name = path.as_os_str().to_owned();
        name.push(".verified.json");
        PathBuf::from(name)
    };
    let source = receipt_path(source);
    match fs::symlink_metadata(&source) {
        Ok(_) => {}
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(problem) => return Err(io_error(problem)),
    }
    let destination = receipt_path(destination);
    if destination.exists() && equal(&source, &destination)? {
        return Ok(());
    }
    publish_copy(&source, &destination.with_extension("next"), &destination)
}

/// DOC-15 §3.2：接续既有安装序号，并按完整内容查找可复用的安装引用。
fn installed_reference(source: &Path, plugins: &Path) -> Result<(Option<String>, u64)> {
    let mut sequence = 0u64;
    for entry in fs::read_dir(plugins).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let installed = name.strip_prefix("installed-");
        let Some(number) = installed
            .or_else(|| name.strip_prefix(".install-"))
            .and_then(|name| name.split('.').next())
        else {
            continue;
        };
        let Ok(number) = number.parse::<u64>() else {
            continue;
        };
        sequence = sequence.max(number);
        if installed.is_some() && equal(source, &entry.path())? {
            return Ok((Some(name.to_owned()), sequence));
        }
    }
    Ok((None, sequence))
}

/// DOC-15 §3.2：复制完整来源后发布引用，失败只清理该次暂存路径。
fn publish_copy(source: &Path, staged: &Path, destination: &Path) -> Result<()> {
    if let Err(problem) = copy(source, staged) {
        let cleanup = if staged.is_dir() {
            fs::remove_dir_all(staged)
        } else {
            fs::remove_file(staged)
        };
        if let Err(cleanup) = cleanup
            && cleanup.kind() != std::io::ErrorKind::NotFound
        {
            return Err(io_error(cleanup));
        }
        return Err(problem);
    }
    fs::rename(staged, destination).map_err(io_error)
}

/// DOC-15 §3.2：目录条目排序用于确定性复制和同名内容比较。
fn children(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(path)
        .map_err(io_error)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(io_error))
        .collect::<Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

/// DOC-15 §3.2：安装副本由普通文件及目录组成，链接和特殊文件在边界返回错误。
fn copy(source: &Path, destination: &Path) -> Result<()> {
    let kind = fs::symlink_metadata(source).map_err(io_error)?.file_type();
    if kind.is_file() {
        fs::copy(source, destination).map_err(io_error)?;
    } else if kind.is_dir() {
        fs::create_dir(destination).map_err(io_error)?;
        for child in children(source)? {
            copy(
                &child,
                &destination.join(child.file_name().expect("directory entry name")),
            )?;
        }
    } else {
        return Err(parameter("插件安装来源应由普通文件与目录组成"));
    }
    Ok(())
}

/// DOC-15 §3.2：按名称递归并分块比较全部字节，读取失败保留文件系统原因。
pub fn equal(left: &Path, right: &Path) -> Result<bool> {
    let left_kind = fs::symlink_metadata(left).map_err(io_error)?.file_type();
    let right_kind = fs::symlink_metadata(right).map_err(io_error)?.file_type();
    if left_kind.is_file() && right_kind.is_file() {
        let mut left = File::open(left).map_err(io_error)?;
        let mut right = File::open(right).map_err(io_error)?;
        if left.metadata().map_err(io_error)?.len() != right.metadata().map_err(io_error)?.len() {
            return Ok(false);
        }
        let mut left_bytes = [0u8; 64 * 1024];
        let mut right_bytes = [0u8; 64 * 1024];
        loop {
            let count = left.read(&mut left_bytes).map_err(io_error)?;
            if count == 0 {
                return Ok(true);
            }
            right
                .read_exact(&mut right_bytes[..count])
                .map_err(io_error)?;
            if left_bytes[..count] != right_bytes[..count] {
                return Ok(false);
            }
        }
    }
    if left_kind.is_dir() && right_kind.is_dir() {
        let left = children(left)?;
        let right = children(right)?;
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(&right) {
            if left.file_name() != right.file_name() || !equal(left, right)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if left_kind.is_symlink() || right_kind.is_symlink() {
        return Err(parameter("插件安装来源应由普通文件与目录组成"));
    }
    Ok(false)
}
