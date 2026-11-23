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
    // 不同状态目录可以共用插件目录。锁覆盖编号分配、复制及验证凭据发布。
    let ownership = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(plugins.join(".install.lock"))
        .map_err(io_error)?;
    ownership.lock().map_err(io_error)?;
    let (existing, sequence) = installed_reference(&source, plugins)?;
    if let Some(reference) = existing {
        let installed = plugins.join(&reference);
        // A copy found after an interrupted publication may be readable but not durable.
        sync_tree(&installed)?;
        sync_path(plugins)?;
        copy_receipt(&source, &installed)?;
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
        sync_tree(&destination)?;
        return sync_path(destination.parent().expect("receipt parent"));
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
    fs::rename(staged, destination).map_err(io_error)?;
    let parent = destination
        .parent()
        .ok_or_else(|| parameter("安装产物缺少父目录"))?;
    sync_path(parent)?;
    if let Some(staged_parent) = staged.parent()
        && staged_parent != parent
    {
        sync_path(staged_parent)?;
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static SYNC_FAILURE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn sync_path(path: &Path) -> Result<()> {
    #[cfg(test)]
    if SYNC_FAILURE.with(|failure| failure.borrow().as_deref() == Some(path)) {
        SYNC_FAILURE.with(|failure| failure.borrow_mut().take());
        return Err(io_error(std::io::Error::other(
            "injected artifact sync failure",
        )));
    }
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)
}

/// Existing artifacts cross the same durable boundary as a newly copied tree.
fn sync_tree(path: &Path) -> Result<()> {
    let kind = fs::symlink_metadata(path).map_err(io_error)?.file_type();
    if kind.is_dir() {
        for child in children(path)? {
            sync_tree(&child)?;
        }
    } else if !kind.is_file() {
        return Err(parameter("插件安装来源应由普通文件与目录组成"));
    }
    sync_path(path)
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
        sync_path(destination)?;
    } else if kind.is_dir() {
        fs::create_dir(destination).map_err(io_error)?;
        for child in children(source)? {
            copy(
                &child,
                &destination.join(child.file_name().expect("directory entry name")),
            )?;
        }
        sync_path(destination)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_testkit::cleanup::TestGuard;
    use std::sync::Barrier;

    #[test]
    fn artifact_sync_failures_never_return_a_reference_and_can_retry() {
        for cut in [
            "file",
            "child-directory",
            "root-directory",
            "publication-parent",
        ] {
            let case = TestGuard::for_package(
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &format!("install-sync-{cut}"),
            );
            let root = case.directory("install-sync");
            fs::create_dir_all(root.join("source/sub")).unwrap();
            fs::create_dir(root.join("plugins")).unwrap();
            let root = root.canonicalize().unwrap();
            let source = root.join("source");
            let plugins = root.join("plugins");
            fs::write(source.join("sub/main.py"), b"ready").unwrap();
            let failed = match cut {
                "file" => plugins.join(".install-1/sub/main.py"),
                "child-directory" => plugins.join(".install-1/sub"),
                "root-directory" => plugins.join(".install-1"),
                _ => plugins.clone(),
            };
            SYNC_FAILURE.with(|failure| *failure.borrow_mut() = Some(failed));
            assert!(install(&source, &root, &root, &plugins).is_err());
            assert!(SYNC_FAILURE.with(|failure| failure.borrow().is_none()));
            let reference = install(&source, &root, &root, &plugins).unwrap();
            assert_eq!(
                fs::read(plugins.join(&reference).join("sub/main.py")).unwrap(),
                b"ready"
            );
            // Reuse also synchronizes the selected artifact before returning its identity.
            SYNC_FAILURE.with(|failure| {
                *failure.borrow_mut() = Some(plugins.join(&reference).join("sub/main.py"))
            });
            assert!(install(&source, &root, &root, &plugins).is_err());
            assert_eq!(install(&source, &root, &root, &plugins).unwrap(), reference);
            case.success();
        }
    }

    #[test]
    fn verification_receipt_is_part_of_successful_publication() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "install-receipt-sync",
        );
        let root = case.directory("install-receipt-sync");
        fs::create_dir_all(root.join("plugins")).unwrap();
        let root = root.canonicalize().unwrap();
        let source = root.join("library.so");
        let plugins = root.join("plugins");
        fs::write(&source, b"library").unwrap();
        fs::write(root.join("library.so.verified.json"), b"receipt").unwrap();
        SYNC_FAILURE.with(|failure| {
            *failure.borrow_mut() = Some(plugins.join("installed-1.so.verified.next"))
        });
        assert!(install(&source, &root, &root, &plugins).is_err());
        assert!(SYNC_FAILURE.with(|failure| failure.borrow().is_none()));
        let reference = install(&source, &root, &root, &plugins).unwrap();
        assert_eq!(reference, "installed-1.so");
        assert_eq!(
            fs::read(plugins.join("installed-1.so.verified.json")).unwrap(),
            b"receipt"
        );
        case.success();
    }

    #[test]
    fn shared_install_directory_keeps_each_source() {
        let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "shared-install");
        let root = case.directory("shared-install");
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let plugins = root.join("plugins");
        fs::create_dir(&plugins).unwrap();
        let sources = [root.join("first.so"), root.join("second.so")];
        for path in &sources {
            fs::write(path, path.file_name().unwrap().as_encoded_bytes()).unwrap();
        }
        let start = Barrier::new(sources.len());
        std::thread::scope(|scope| {
            let jobs: Vec<_> = sources
                .iter()
                .map(|path| {
                    scope.spawn(|| {
                        start.wait();
                        install(path, &root, &root, &plugins).unwrap()
                    })
                })
                .collect();
            for (job, source) in jobs.into_iter().zip(&sources) {
                let installed = plugins.join(job.join().unwrap());
                assert_eq!(fs::read(installed).unwrap(), fs::read(source).unwrap());
            }
        });
        case.success();
    }
}
