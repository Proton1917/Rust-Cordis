//! A rebuildable candidate index. Published references remain owned by their
//! state roots, rollback/recovery records and candidates, not by this cache.

use super::{Result, artifact_error, children, equal, io_error, parameter, sync_path};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io::Read,
    os::unix::fs::MetadataExt,
    path::Path,
};

const INDEX: &str = ".install-index-v1.json";
const VERSION: u32 = 1;
type Hash = [u8; 32];

#[derive(Deserialize, Serialize)]
struct Stored<R = BTreeMap<String, Record>> {
    version: u32,
    records: R,
}

#[derive(Clone, Deserialize, Serialize)]
struct Record {
    stamp: Hash,
    content: Hash,
}

pub(super) struct Inventory {
    records: BTreeMap<String, Record>,
    candidates: HashMap<Hash, Vec<String>>,
    pub sequence: u64,
    dirty: bool,
}

impl Inventory {
    /// The caller holds the shared plugin-directory lock for this whole lifetime.
    pub fn open(plugins: &Path) -> Result<Self> {
        let cached = crate::storage::read(&plugins.join(INDEX))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .filter(|stored| stored.version == VERSION);
        let mut inventory = Self {
            records: BTreeMap::new(),
            candidates: HashMap::new(),
            sequence: 0,
            dirty: cached.is_none(),
        };
        for entry in fs::read_dir(plugins).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let installed = name.strip_prefix("installed-");
            let Some(number) = installed
                .or_else(|| name.strip_prefix(".install-"))
                .and_then(|suffix| suffix.split('.').next())
                .and_then(|number| number.parse::<u64>().ok())
            else {
                continue;
            };
            inventory.sequence = inventory.sequence.max(number);
            // Receipts belong to the artifact publication, and never become an
            // independently installable content candidate.
            if installed.is_none()
                || name.ends_with(".verified.json")
                || name.ends_with(".verified.next")
            {
                continue;
            }
            let path = entry.path();
            let stamp = fingerprint(&path)?;
            let record = match cached.as_ref().and_then(|stored| stored.records.get(name)) {
                Some(record) if record.stamp == stamp => record.clone(),
                _ => {
                    inventory.dirty = true;
                    record(&path, stamp)?
                }
            };
            inventory
                .candidates
                .entry(record.content)
                .or_default()
                .push(name.into());
            inventory.records.insert(name.into(), record);
        }
        if cached
            .as_ref()
            .is_some_and(|old| old.records.len() != inventory.records.len())
        {
            inventory.dirty = true;
        }
        // Stable ordering makes equally valid duplicate references deterministic.
        for references in inventory.candidates.values_mut() {
            references.sort();
        }
        Ok(inventory)
    }

    pub fn find(&self, source: &Path, plugins: &Path) -> Result<Option<String>> {
        if self.candidates.is_empty() {
            return Ok(None);
        }
        let content = content(source)?;
        for reference in self.candidates.get(&content).into_iter().flatten() {
            #[cfg(test)]
            CANDIDATES.with(|count| count.set(count.get() + 1));
            // The digest is only a locator: a collision or stale cache cannot
            // replace the complete tree/byte identity required by install.
            if equal(source, &plugins.join(reference))? {
                return Ok(Some(reference.clone()));
            }
        }
        Ok(None)
    }

    pub fn insert(&mut self, reference: &str, plugins: &Path) -> Result<()> {
        let path = plugins.join(reference);
        self.records
            .insert(reference.into(), record(&path, fingerprint(&path)?)?);
        self.dirty = true;
        Ok(())
    }

    pub fn save(&self, plugins: &Path) -> Result<()> {
        if self.dirty {
            let bytes = serde_json::to_vec(&Stored {
                version: VERSION,
                records: &self.records,
            })
            .map_err(parameter)?;
            crate::storage::write(&plugins.join(INDEX), &bytes)?;
            sync_path(plugins)?;
        }
        Ok(())
    }
}

fn record(path: &Path, stamp: Hash) -> Result<Record> {
    #[cfg(test)]
    REHASHES.with(|count| count.set(count.get() + 1));
    cordis_artifact::validate(path, &BTreeSet::new(), &[]).map_err(artifact_error)?;
    let content = content(path)?;
    if fingerprint(path)? != stamp {
        return Err(parameter("安装产物在建立索引时发生变化"));
    }
    Ok(Record { stamp, content })
}

fn fingerprint(path: &Path) -> Result<Hash> {
    let root = path.canonicalize().map_err(io_error)?;
    let mut hash = Sha256::new();
    walk(path, &root, &mut hash, false)?;
    Ok(hash.finalize().into())
}

fn content(path: &Path) -> Result<Hash> {
    let root = path.canonicalize().map_err(io_error)?;
    let mut hash = Sha256::new();
    walk(path, &root, &mut hash, true)?;
    Ok(hash.finalize().into())
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

/// Metadata is an invalidation hint, never identity. Every file's ctime/mtime,
/// inode and length are included, so nested modifications invalidate the cache.
/// Content and equality both use the same normalized internal-link target.
fn walk(path: &Path, root: &Path, hash: &mut Sha256, bytes: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    let kind = metadata.file_type();
    if !bytes {
        for value in [
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64,
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u64,
        ] {
            hash.update(value.to_le_bytes());
        }
    }
    if kind.is_file() {
        hash.update([0]);
        hash.update(metadata.len().to_le_bytes());
        if bytes {
            hash_file(path, hash)?;
        }
    } else if kind.is_dir() {
        hash.update([1]);
        let children = children(path)?;
        hash.update((children.len() as u64).to_le_bytes());
        for child in children {
            field(
                hash,
                child
                    .file_name()
                    .expect("directory entry")
                    .as_encoded_bytes(),
            );
            walk(&child, root, hash, bytes)?;
        }
    } else if kind.is_symlink() {
        hash.update([2]);
        let target = cordis_artifact::link_target(path, root).map_err(artifact_error)?;
        field(hash, target.as_os_str().as_encoded_bytes());
    } else {
        return Err(parameter("插件安装来源包含特殊文件"));
    }
    Ok(())
}

// Keep the I/O buffer off recursive directory and metadata-only stack frames.
fn hash_file(path: &Path, hash: &mut Sha256) -> Result<()> {
    #[cfg(test)]
    HASHED_FILES.with(|count| count.set(count.get() + 1));
    let mut file = File::open(path).map_err(io_error)?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            return Ok(());
        }
        #[cfg(test)]
        HASHED_BYTES.with(|total| total.set(total.get() + count as u64));
        hash.update(&buffer[..count]);
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static REHASHES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(super) static CANDIDATES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(super) static HASHED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(super) static HASHED_FILES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{COMPARED_BYTES, COMPARED_FILES, install};
    use cordis_testkit::cleanup::TestGuard;

    #[test]
    fn missing_corrupt_and_obsolete_indexes_rebuild_and_detect_nested_changes() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "install-index-rebuild",
        );
        let root = case.directory("install-index-rebuild");
        fs::create_dir_all(root.join("source/nested")).unwrap();
        fs::create_dir(root.join("plugins")).unwrap();
        let root = root.canonicalize().unwrap();
        let source = root.join("source");
        let plugins = root.join("plugins");
        fs::write(source.join("nested/data"), b"before").unwrap();
        let first = install(&source, &root, &root, &plugins).unwrap();
        // Editing a nested file leaves the root directory's metadata untouched.
        fs::write(plugins.join(&first).join("nested/data"), b"after!").unwrap();
        fs::write(source.join("nested/data"), b"after!").unwrap();
        assert_eq!(install(&source, &root, &root, &plugins).unwrap(), first);
        for bytes in [
            None,
            Some(b"{".as_slice()),
            Some(b"{\"version\":2,\"records\":{}}".as_slice()),
        ] {
            if let Some(bytes) = bytes {
                fs::write(plugins.join(INDEX), bytes).unwrap();
            } else {
                fs::remove_file(plugins.join(INDEX)).unwrap();
            }
            assert_eq!(install(&source, &root, &root, &plugins).unwrap(), first);
        }
        // An offline/restored reference and an abandoned staging number both
        // participate in allocation, independently of the cached inventory.
        super::super::copy(&source, &plugins.join("installed-40")).unwrap();
        fs::create_dir(plugins.join(".install-41")).unwrap();
        fs::write(source.join("nested/data"), b"change").unwrap();
        let next = install(&source, &root, &root, &plugins).unwrap();
        assert_eq!(next, "installed-42");
        assert!(plugins.join(first).is_dir());
        assert!(plugins.join("installed-40").is_dir());
        case.success();
    }

    #[test]
    fn a_matching_digest_hint_still_requires_complete_byte_identity() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "install-index-hint");
        let root = case.directory("install-index-hint");
        fs::create_dir_all(root.join("plugins")).unwrap();
        let root = root.canonicalize().unwrap();
        let source = root.join("source.so");
        let plugins = root.join("plugins");
        fs::write(&source, b"original").unwrap();
        let first = install(&source, &root, &root, &plugins).unwrap();
        fs::write(&source, b"distinct").unwrap();
        let mut stored: Stored =
            serde_json::from_slice(&fs::read(plugins.join(INDEX)).unwrap()).unwrap();
        stored.records.get_mut(&first).unwrap().content = content(&source).unwrap();
        fs::write(plugins.join(INDEX), serde_json::to_vec(&stored).unwrap()).unwrap();
        CANDIDATES.with(|count| count.set(0));
        let next = install(&source, &root, &root, &plugins).unwrap();
        assert_ne!(first, next);
        assert_eq!(CANDIDATES.with(|count| count.get()), 1);
        assert_eq!(fs::read(plugins.join(&first)).unwrap(), b"original");
        assert_eq!(fs::read(plugins.join(next)).unwrap(), b"distinct");
        case.success();
    }

    #[test]
    fn warm_inventory_reads_only_source_and_selected_candidate_content() {
        const SIZE: usize = 64 * 1024;
        for count in [10, 100, 1_000] {
            let case = TestGuard::for_package(
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &format!("install-index-{count}"),
            );
            let root = case.directory("install-index-history");
            fs::create_dir_all(root.join("plugins")).unwrap();
            let root = root.canonicalize().unwrap();
            let plugins = root.join("plugins");
            let source = root.join("source.so");
            let mut bytes = vec![0; SIZE];
            for number in 1..=count {
                bytes[SIZE - 8..].copy_from_slice(&(number as u64).to_le_bytes());
                fs::write(plugins.join(format!("installed-{number}.so")), &bytes).unwrap();
            }
            fs::write(&source, &bytes).unwrap();
            let expected = format!("installed-{count}.so");
            // Prime the on-disk index through the actual install boundary.
            assert_eq!(install(&source, &root, &root, &plugins).unwrap(), expected);
            COMPARED_BYTES.with(|total| total.set(0));
            COMPARED_FILES.with(|total| total.set(0));
            for number in 1..=count {
                if equal(&source, &plugins.join(format!("installed-{number}.so"))).unwrap() {
                    break;
                }
            }
            let baseline_bytes = COMPARED_BYTES.with(|total| total.get());
            let baseline_opens = COMPARED_FILES.with(|total| total.get());
            assert_eq!(baseline_bytes, 2 * count as u64 * SIZE as u64);
            COMPARED_BYTES.with(|total| total.set(0));
            COMPARED_FILES.with(|total| total.set(0));
            HASHED_BYTES.with(|total| total.set(0));
            HASHED_FILES.with(|total| total.set(0));
            REHASHES.with(|total| total.set(0));
            CANDIDATES.with(|total| total.set(0));
            assert_eq!(install(&source, &root, &root, &plugins).unwrap(), expected);
            assert_eq!(REHASHES.with(|total| total.get()), 0);
            assert_eq!(CANDIDATES.with(|total| total.get()), 1);
            let indexed_bytes =
                HASHED_BYTES.with(|total| total.get()) + COMPARED_BYTES.with(|total| total.get());
            let indexed_opens =
                HASHED_FILES.with(|total| total.get()) + COMPARED_FILES.with(|total| total.get());
            assert_eq!(indexed_bytes, 3 * SIZE as u64);
            assert_eq!(indexed_opens, 3);
            eprintln!(
                "install-index history={count} baseline_content_opens={baseline_opens} indexed_content_opens={indexed_opens} baseline_read_bytes={baseline_bytes} indexed_read_bytes={indexed_bytes} historical_rehashes=0 candidates=1"
            );
            case.success();
        }
    }
}
