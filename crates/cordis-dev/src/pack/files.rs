//! ADR-0036：发布树保存运行文件和包内链接，归档成功后释放本次装配目录。

use crate::Result;
use flate2::{Compression, write::GzEncoder};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// DOC-15 §12.2：逐目录复制；符号链接按实际目标重新写为包内相对链接。
pub fn copy_tree(source: &Path, destination: &Path, excluded: &BTreeSet<PathBuf>) -> Result<()> {
    let root = source.canonicalize()?;
    fs::create_dir_all(destination)?;
    if destination.canonicalize()?.starts_with(&root) {
        return Err(format!("装配目录位于复制来源内：{}", root.display()).into());
    }
    cordis_artifact::validate(&root, excluded, &[".git", "__pycache__"])?;
    copy_entries(&root, destination, &root, excluded)?;
    cordis_artifact::validate(destination, &BTreeSet::new(), &[])?;
    Ok(())
}

fn copy_entries(
    source: &Path,
    destination: &Path,
    root: &Path,
    excluded: &BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git" | "__pycache__")) {
            continue;
        }
        let target = destination.join(&name);
        let kind = entry.file_type()?;
        if cordis_artifact::excluded(&entry.path(), root, excluded, &[".git", "__pycache__"])? {
            continue;
        }
        if kind.is_symlink() {
            let link = cordis_artifact::relative_link(&entry.path(), root)?;
            create_symlink(&entry.path(), &link, &target)?;
        } else if kind.is_dir() {
            fs::create_dir(&target)?;
            copy_entries(&entry.path(), &target, root, excluded)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(format!("发布树包含特殊文件：{}", entry.path().display()).into());
        }
    }
    Ok(())
}

/// AU-12：归档包含一个发布根目录；成功交付后移除可重建装配树。
pub fn archive(source: &Path, out: &Path) -> Result<PathBuf> {
    let name = source.file_name().ok_or("发布目录需要名称")?;
    let archive = out.join(name).with_added_extension("tar.gz");
    let staged = archive.with_added_extension(format!("{}.partial", std::process::id()));
    let output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    let publish = || -> Result<()> {
        let mut builder = tar::Builder::new(GzEncoder::new(output, Compression::default()));
        builder.follow_symlinks(false);
        builder.append_dir_all(name, source)?;
        let file = builder.into_inner()?.finish()?;
        file.sync_all()?;
        drop(file);
        rename_noreplace(&staged, &archive)?;
        Ok(())
    };
    if let Err(problem) = publish() {
        fs::remove_file(staged)?;
        return Err(problem);
    }
    fs::remove_dir_all(source)?;
    Ok(archive)
}

#[cfg(unix)]
fn create_symlink(_source: &Path, link: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(link, target)
}

#[cfg(windows)]
fn create_symlink(source: &Path, link: &Path, target: &Path) -> std::io::Result<()> {
    if fs::metadata(source)?.is_dir() {
        std::os::windows::fs::symlink_dir(link, target)
    } else {
        std::os::windows::fs::symlink_file(link, target)
    }
}

#[cfg(unix)]
fn rename_noreplace(from: &Path, to: &Path) -> Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE)?;
    Ok(())
}

#[cfg(windows)]
fn rename_noreplace(from: &Path, to: &Path) -> Result<()> {
    // NTFS 硬链接在目标已存在时原子返回 AlreadyExists，随后移除暂存名，保持零 unsafe。
    fs::hard_link(from, to)?;
    fs::remove_file(from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_testkit::cleanup::TestGuard;

    #[test]
    fn archive_preserves_internal_links_with_the_installable_tree_policy() {
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
        let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "pack-links");
        let root = case.directory("pack-links");
        let source = root.join("source");
        fs::create_dir_all(source.join("data")).unwrap();
        let source = source.canonicalize().unwrap();
        fs::write(source.join("data/file"), b"value").unwrap();
        link_file(&source.join("data/file"), &source.join("alias")).unwrap();
        link_dir(Path::new("data"), &source.join("directory")).unwrap();
        let secret = root.join("excluded-secret");
        fs::write(&secret, b"secret").unwrap();
        link_file(&secret, &source.join("secret-alias")).unwrap();
        let staged = root.join("artifact");
        copy_tree(
            &source,
            &staged,
            &BTreeSet::from([secret.canonicalize().unwrap()]),
        )
        .unwrap();
        assert!(!staged.join("secret-alias").exists());
        fs::remove_file(source.join("secret-alias")).unwrap();
        assert_eq!(
            fs::read_link(staged.join("alias")).unwrap(),
            Path::new("data").join("file")
        );
        let archive = archive(&staged, &root).unwrap();
        let extracted = root.join("extracted");
        tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(archive).unwrap(),
        ))
        .unpack(&extracted)
        .unwrap();
        let artifact = extracted.join("artifact");
        cordis_artifact::validate(&artifact, &BTreeSet::new(), &[]).unwrap();
        assert_eq!(fs::read(artifact.join("directory/file")).unwrap(), b"value");
        link_dir(Path::new(".."), &source.join("data/cycle")).unwrap();
        assert!(copy_tree(&source, &root.join("cycle"), &BTreeSet::new()).is_err());
        case.success();
    }

    #[test]
    fn packaging_rejects_nested_outputs_and_publishes_only_complete_archives() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "pack-publication");
        let work = case.directory("pack-publication");
        let source = work.join("application");
        fs::create_dir_all(&source).unwrap();
        assert!(copy_tree(&source, &source.join("out"), &BTreeSet::new()).is_err());
        let missing = work.join("missing");
        assert!(archive(&missing, &work).is_err());
        assert!(!work.join("missing.tar.gz").exists());
        let previous = work.join("application.tar.gz");
        fs::write(&previous, b"published").unwrap();
        assert!(archive(&source, &work).is_err());
        assert_eq!(fs::read(previous).unwrap(), b"published");
        case.success();
    }
}
