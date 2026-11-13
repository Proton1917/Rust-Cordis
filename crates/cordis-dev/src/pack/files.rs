//! ADR-0036：发布树保存运行文件和包内链接，归档成功后释放本次装配目录。

use crate::Result;
use flate2::{Compression, write::GzEncoder};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// DOC-15 §12.2：逐目录复制；符号链接按实际目标重新写为包内相对链接。
pub fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let root = source.canonicalize()?;
    fs::create_dir_all(destination)?;
    copy_entries(&root, destination, &root, destination)
}

fn copy_entries(source: &Path, destination: &Path, root: &Path, output: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git" | "__pycache__")) {
            continue;
        }
        let target = destination.join(&name);
        let kind = entry.file_type()?;
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
            copy_entries(&entry.path(), &target, root, output)?;
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
    let output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&archive)?;
    let encoder = GzEncoder::new(output, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    builder.append_dir_all(name, source)?;
    let encoder = builder.into_inner()?;
    encoder.finish()?.sync_all()?;
    fs::remove_dir_all(source)?;
    Ok(archive)
}
