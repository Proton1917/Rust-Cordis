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
    copy_entries(&root, destination, &root, destination, excluded)
}

fn copy_entries(
    source: &Path,
    destination: &Path,
    root: &Path,
    output: &Path,
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
        if !kind.is_dir() && excluded.contains(&entry.path().canonicalize()?) {
            continue;
        }
        if kind.is_symlink() {
            let resolved = entry.path().canonicalize()?;
            let relative = resolved
                .strip_prefix(root)
                .map_err(|_| format!("发布链接指向来源目录之外：{}", entry.path().display()))?;
            let link = pathdiff::diff_paths(output.join(relative), destination)
                .ok_or("链接目标与发布目录需要共同根目录")?;
            std::os::unix::fs::symlink(link, target)?;
        } else if kind.is_dir() {
            fs::create_dir(&target)?;
            copy_entries(&entry.path(), &target, root, output, excluded)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
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
        builder.into_inner()?.finish()?.sync_all()?;
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        renameat_with(CWD, &staged, CWD, &archive, RenameFlags::NOREPLACE)?;
        Ok(())
    };
    if let Err(problem) = publish() {
        fs::remove_file(staged)?;
        return Err(problem);
    }
    fs::remove_dir_all(source)?;
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_testkit::cleanup::TestGuard;

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
