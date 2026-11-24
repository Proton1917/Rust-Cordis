//! Shared file-tree closure for packed and installed plugin artifacts.
//!
//! An artifact contains regular files, directories, and links whose fully resolved
//! targets remain inside the root. Directory links participate in cycle checking.

use std::{
    collections::{BTreeSet, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// Return the canonical target relative to the artifact root.
pub fn link_target(path: &Path, root: &Path) -> io::Result<PathBuf> {
    let resolved = path.canonicalize()?;
    resolved
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| {
            invalid(format!(
                "artifact link escapes its root: {}",
                path.display()
            ))
        })
}

/// Relocatable link spelling shared by the packer, installer and equality check.
pub fn relative_link(path: &Path, root: &Path) -> io::Result<PathBuf> {
    let target = root.join(link_target(path, root)?);
    pathdiff::diff_paths(
        target,
        path.parent().ok_or_else(|| invalid("link has no parent"))?,
    )
    .ok_or_else(|| invalid("link target and artifact need a common root"))
}

/// The same exclusion closure is applied during validation and packing.
pub fn excluded(
    path: &Path,
    root: &Path,
    excluded: &BTreeSet<PathBuf>,
    ignored: &[&str],
) -> io::Result<bool> {
    let resolved = path.canonicalize()?;
    if excluded
        .iter()
        .any(|excluded| resolved.starts_with(excluded))
    {
        return Ok(true);
    }
    let relative = resolved.strip_prefix(root).map_err(|_| {
        invalid(format!(
            "artifact link escapes its root: {}",
            path.display()
        ))
    })?;
    Ok(relative
        .components()
        .any(|part| ignored.iter().any(|name| part.as_os_str() == *name)))
}

/// Validate the reachable file tree once, including directory links.
/// Excluded trees and ignored build metadata do not become artifact members.
pub fn validate(
    root: &Path,
    excluded: &BTreeSet<PathBuf>,
    ignored_names: &[&str],
) -> io::Result<()> {
    let root = root.canonicalize()?;
    let mut visited = HashSet::new();
    let mut active = HashSet::new();
    visit(
        &root,
        &root,
        excluded,
        ignored_names,
        &mut visited,
        &mut active,
    )
}

fn visit(
    path: &Path,
    root: &Path,
    excluded: &BTreeSet<PathBuf>,
    ignored: &[&str],
    visited: &mut HashSet<PathBuf>,
    active: &mut HashSet<PathBuf>,
) -> io::Result<()> {
    let resolved = path.canonicalize()?;
    if excluded
        .iter()
        .any(|excluded| resolved.starts_with(excluded))
    {
        return Ok(());
    }
    if !resolved.starts_with(root) {
        return Err(invalid(format!(
            "artifact link escapes its root: {}",
            path.display()
        )));
    }
    let relative = resolved.strip_prefix(root).expect("validated root");
    if relative
        .components()
        .any(|part| ignored.iter().any(|name| part.as_os_str() == *name))
    {
        return Ok(());
    }
    let kind = fs::metadata(&resolved)?.file_type();
    if kind.is_file() {
        return Ok(());
    }
    if !kind.is_dir() {
        return Err(invalid(format!(
            "artifact contains a special file: {}",
            path.display()
        )));
    }
    if active.contains(&resolved) {
        return Err(invalid(format!(
            "artifact directory link forms a cycle: {}",
            path.display()
        )));
    }
    if !visited.insert(resolved.clone()) {
        return Ok(());
    }
    active.insert(resolved.clone());
    for entry in fs::read_dir(&resolved)? {
        let entry = entry?;
        if ignored.iter().any(|name| entry.file_name() == *name) {
            continue;
        }
        visit(&entry.path(), root, excluded, ignored, visited, active)?;
    }
    active.remove(&resolved);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_testkit::cleanup::TestGuard;
    use std::os::unix::fs::symlink;

    #[test]
    fn internal_links_are_relocatable_and_directory_cycles_are_rejected() {
        let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "artifact-links");
        let root = case.directory("artifact-links");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir(root.join("b")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("b/file"), b"payload").unwrap();
        symlink(root.join("b/file"), root.join("a/alias")).unwrap();
        symlink("../b", root.join("a/dir")).unwrap();
        validate(&root, &BTreeSet::new(), &[]).unwrap();
        assert_eq!(
            relative_link(&root.join("a/alias"), &root).unwrap(),
            Path::new("../b/file")
        );
        symlink("../a", root.join("b/back")).unwrap();
        assert_eq!(
            validate(&root, &BTreeSet::new(), &[]).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        fs::remove_file(root.join("b/back")).unwrap();
        symlink("../..", root.join("a/outside")).unwrap();
        assert_eq!(
            validate(&root, &BTreeSet::new(), &[]).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        fs::remove_file(root.join("a/outside")).unwrap();
        symlink("loop-two", root.join("loop-one")).unwrap();
        symlink("loop-one", root.join("loop-two")).unwrap();
        assert!(validate(&root, &BTreeSet::new(), &[]).is_err());
        case.success();
    }
}
