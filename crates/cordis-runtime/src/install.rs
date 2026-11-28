//! DOC-15 §3.2：内容索引定位候选，完整字节比较决定安装引用的复用。

mod index;

use crate::{
    config::{Result, parameter},
    storage::io_error,
};
use std::{
    collections::BTreeSet,
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
    cordis_artifact::validate(&source, &BTreeSet::new(), &[]).map_err(artifact_error)?;
    // 不同状态目录可以共用插件目录。锁覆盖编号分配、复制及验证凭据发布。
    let ownership = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(plugins.join(".install.lock"))
        .map_err(io_error)?;
    ownership.lock().map_err(io_error)?;
    let mut inventory = index::Inventory::open(plugins)?;
    if let Some(reference) = inventory.find(&source, plugins)? {
        let installed = plugins.join(&reference);
        // A copy found after an interrupted publication may be readable but not durable.
        sync_tree(&installed)?;
        sync_path(plugins)?;
        copy_receipt(&source, &installed)?;
        inventory.save(plugins)?;
        return Ok(reference);
    }
    let next = inventory
        .sequence
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
    inventory.insert(&reference, plugins)?;
    inventory.save(plugins)?;
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
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(parameter("验证凭据需要普通文件")),
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

/// DOC-15 §3.2：复制完整来源后发布引用，失败只清理该次暂存路径。
fn publish_copy(source: &Path, staged: &Path, destination: &Path) -> Result<()> {
    if let Err(problem) = copy(source, staged).and_then(|()| {
        cordis_artifact::validate(staged, &BTreeSet::new(), &[]).map_err(artifact_error)
    }) {
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
    static COMPARED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static COMPARED_FILES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn sync_path(path: &Path) -> Result<()> {
    #[cfg(test)]
    if SYNC_FAILURE.with(|failure| failure.borrow().as_deref() == Some(path)) {
        SYNC_FAILURE.with(|failure| failure.borrow_mut().take());
        return Err(io_error(std::io::Error::other(
            "injected artifact sync failure",
        )));
    }
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(io_error)
    }
    #[cfg(windows)]
    {
        if path.is_dir() {
            return Ok(());
        }
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .and_then(|file| file.sync_all())
            .map_err(io_error)
    }
}

/// Existing artifacts cross the same durable boundary as a newly copied tree.
fn sync_tree(path: &Path) -> Result<()> {
    let kind = fs::symlink_metadata(path).map_err(io_error)?.file_type();
    if kind.is_dir() {
        for child in children(path)? {
            sync_tree(&child)?;
        }
    } else if kind.is_symlink() {
        // The containing directory synchronizes this entry; its target is another
        // validated member whose contents are synchronized by the regular walk.
        return Ok(());
    } else if !kind.is_file() {
        return Err(parameter("插件安装来源包含特殊文件"));
    }
    sync_path(path)
}

fn artifact_error(problem: std::io::Error) -> cordis_up::WireError {
    if problem.kind() == std::io::ErrorKind::InvalidInput {
        parameter(problem)
    } else {
        io_error(problem)
    }
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

/// The root is a regular source; internal links use the shared relocatable policy.
fn copy(source: &Path, destination: &Path) -> Result<()> {
    if fs::symlink_metadata(source)
        .map_err(io_error)?
        .file_type()
        .is_symlink()
    {
        return Err(parameter("安装根或验证凭据需要普通文件或目录"));
    }
    copy_entry(
        source,
        destination,
        &source.canonicalize().map_err(io_error)?,
    )
}

fn copy_entry(source: &Path, destination: &Path, root: &Path) -> Result<()> {
    let kind = fs::symlink_metadata(source).map_err(io_error)?.file_type();
    if kind.is_file() {
        fs::copy(source, destination).map_err(io_error)?;
        sync_path(destination)?;
    } else if kind.is_dir() {
        fs::create_dir(destination).map_err(io_error)?;
        for child in children(source)? {
            copy_entry(
                &child,
                &destination.join(child.file_name().expect("directory entry name")),
                root,
            )?;
        }
        sync_path(destination)?;
    } else if kind.is_symlink() {
        let target = cordis_artifact::relative_link(source, root).map_err(artifact_error)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination).map_err(io_error)?;
        #[cfg(windows)]
        {
            if fs::metadata(source).map_err(io_error)?.is_dir() {
                std::os::windows::fs::symlink_dir(target, destination).map_err(io_error)?;
            } else {
                std::os::windows::fs::symlink_file(target, destination).map_err(io_error)?;
            }
        }
    } else {
        return Err(parameter("插件安装来源包含特殊文件"));
    }
    Ok(())
}

/// DOC-15 §3.2：按名称递归并分块比较全部字节，读取失败保留文件系统原因。
pub fn equal(left: &Path, right: &Path) -> Result<bool> {
    let left_root = left.canonicalize().map_err(io_error)?;
    let right_root = right.canonicalize().map_err(io_error)?;
    equal_entry(left, right, &left_root, &right_root)
}

fn equal_entry(left: &Path, right: &Path, left_root: &Path, right_root: &Path) -> Result<bool> {
    let left_kind = fs::symlink_metadata(left).map_err(io_error)?.file_type();
    let right_kind = fs::symlink_metadata(right).map_err(io_error)?.file_type();
    if left_kind.is_file() && right_kind.is_file() {
        #[cfg(test)]
        COMPARED_FILES.with(|count| count.set(count.get() + 2));
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
            #[cfg(test)]
            COMPARED_BYTES.with(|total| total.set(total.get() + 2 * count as u64));
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
            if left.file_name() != right.file_name()
                || !equal_entry(left, right, left_root, right_root)?
            {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if left_kind.is_symlink() && right_kind.is_symlink() {
        return Ok(
            cordis_artifact::link_target(left, left_root).map_err(artifact_error)?
                == cordis_artifact::link_target(right, right_root).map_err(artifact_error)?,
        );
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_testkit::cleanup::TestGuard;
    use std::sync::Barrier;

    #[test]
    fn internal_file_and_directory_links_keep_identity_after_install_and_reuse() {
        let link_file = |src: &Path, dst: &Path| -> std::io::Result<()> {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(src, dst)
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(src, dst)
            }
        };
        let link_dir = |src: &Path, dst: &Path| -> std::io::Result<()> {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(src, dst)
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_dir(src, dst)
            }
        };
        let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "install-links");
        let root = case.directory("install-links");
        fs::create_dir_all(root.join("source/data")).unwrap();
        fs::create_dir(root.join("plugins")).unwrap();
        let root = root.canonicalize().unwrap();
        let source = root.join("source");
        let plugins = root.join("plugins");
        fs::write(source.join("data/first"), b"one").unwrap();
        fs::write(source.join("data/second"), b"two").unwrap();
        link_file(&source.join("data/first"), &source.join("alias")).unwrap();
        link_dir(Path::new("data"), &source.join("directory")).unwrap();
        let first = install(&source, &root, &root, &plugins).unwrap();
        let installed = plugins.join(&first);
        assert_eq!(
            fs::read_link(installed.join("alias")).unwrap(),
            Path::new("data").join("first")
        );
        assert_eq!(fs::read(installed.join("directory/first")).unwrap(), b"one");
        assert!(equal(&source, &installed).unwrap());
        assert_eq!(install(&source, &root, &root, &plugins).unwrap(), first);
        fs::remove_file(source.join("alias")).unwrap();
        link_file(&Path::new("data").join("second"), &source.join("alias")).unwrap();
        assert!(!equal(&source, &installed).unwrap());
        assert_ne!(install(&source, &root, &root, &plugins).unwrap(), first);
        link_dir(Path::new(".."), &source.join("data/cycle")).unwrap();
        assert!(install(&source, &root, &root, &plugins).is_err());
        case.success();
    }

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
        let project = root.join("project");
        let data = [root.join("first-state"), root.join("second-state")];
        fs::create_dir(&project).unwrap();
        let sources = data.each_ref().map(|directory| {
            fs::create_dir(directory).unwrap();
            let source = directory.join("plugin.so");
            fs::write(&source, directory.file_name().unwrap().as_encoded_bytes()).unwrap();
            source
        });
        let start = Barrier::new(sources.len());
        let references = std::thread::scope(|scope| {
            let jobs: Vec<_> = sources
                .iter()
                .zip(&data)
                .map(|(path, data)| {
                    scope.spawn(|| {
                        start.wait();
                        install(path, &project, data, &plugins).unwrap()
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        for (reference, source) in references.iter().zip(&sources) {
            assert_eq!(
                fs::read(plugins.join(reference)).unwrap(),
                fs::read(source).unwrap()
            );
        }
        fs::write(&sources[0], b"new-first-state").unwrap();
        let replacement = install(&sources[0], &project, &data[0], &plugins).unwrap();
        assert_ne!(replacement, references[0]);
        assert_eq!(
            fs::read(plugins.join(&references[0])).unwrap(),
            b"first-state"
        );
        assert_eq!(
            fs::read(plugins.join(&references[1])).unwrap(),
            b"second-state"
        );
        assert_eq!(
            install(&sources[1], &project, &data[1], &plugins).unwrap(),
            references[1]
        );
        case.success();
    }
}
