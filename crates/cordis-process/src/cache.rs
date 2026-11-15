//! ADR-0053：共享文件租约保护副本与重启来源，排他租约负责进程间回收。

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const LOCK_TIMEOUT: Duration = Duration::from_secs(1);

pub struct CacheDirectory(PathBuf);

pub struct CachePin {
    directory: PathBuf,
    _file: File,
}

pub struct CacheCopy {
    path: PathBuf,
    pin: CachePin,
}

impl CacheDirectory {
    pub fn new(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        Ok(Self(directory.canonicalize()?))
    }

    pub fn directory(&self) -> &Path {
        &self.0
    }

    /// SW-1：目录锁内先回收无引用项，再持久分配序号和共享租约。
    pub fn reserve(&self, extension: &str) -> io::Result<(u64, CacheCopy, File)> {
        let deadline = Instant::now() + LOCK_TIMEOUT;
        let directory_lock = lock_directory(&self.0, deadline)?;
        if let Err(error) = prune_locked(&self.0) {
            eprintln!("库副本回收失败：{error}");
        }
        let sequence = next_sequence(&self.0)?;
        let path = self.0.join(format!("candidate-{sequence}.{extension}"));
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker_path(&self.0, sequence))?;
        fs::hard_link(marker_path(&self.0, sequence), &path)?;
        let marker = File::open(marker_path(&self.0, sequence))?;
        lock_file(&marker, true, deadline)?;
        drop(directory_lock);
        Ok((
            sequence,
            CacheCopy {
                path,
                pin: CachePin {
                    directory: self.0.clone(),
                    _file: marker,
                },
            },
            output,
        ))
    }

    /// CP-6：操作系统已释放崩溃进程的文件锁，继续保护其他进程持有的副本。
    pub fn prune(&self) -> io::Result<usize> {
        let _lock = lock_directory(&self.0, Instant::now() + LOCK_TIMEOUT)?;
        prune_locked(&self.0)
    }
}

impl CacheCopy {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// SW-1：库名由宿主描述符校验，序号锁使发布与回收保持相同次序。
    pub fn publish(&mut self, name: &str) -> io::Result<PathBuf> {
        let _lock = lock_directory(&self.pin.directory, Instant::now() + LOCK_TIMEOUT)?;
        let path = self.pin.directory.join(name);
        fs::rename(&self.path, &path)?;
        self.path = path.clone();
        Ok(path)
    }
}

impl CachePin {
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// ADR-0053：缓存来源取得共享文件租约；历史副本和普通部署输入保持原存储寿命。
    pub fn open(path: &Path) -> io::Result<Option<Self>> {
        Self::open_until(path, Instant::now() + LOCK_TIMEOUT)
    }

    fn open_until(path: &Path, deadline: Instant) -> io::Result<Option<Self>> {
        let Some(sequence) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(copy_sequence)
        else {
            return Ok(None);
        };
        let Some(directory) = path.parent() else {
            return Ok(None);
        };
        let marker = marker_path(directory, sequence);
        if !marker.try_exists()? {
            return Ok(None);
        }
        let _lock = lock_directory(directory, deadline)?;
        let file = match File::open(&marker) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !same_file(&file.metadata()?, &fs::metadata(path)?) {
            return Ok(None);
        }
        lock_file(&file, true, deadline)?;
        Ok(Some(Self {
            directory: directory.to_owned(),
            _file: file,
        }))
    }
}

/// ADR-0053：文件 I/O 交给阻塞执行入口，整个来源集合共用调用方的截止时间。
pub async fn pin_sources(paths: Vec<PathBuf>, timeout: Duration) -> io::Result<Vec<CachePin>> {
    let deadline = Instant::now() + timeout;
    blocking(timeout, move || {
        paths
            .iter()
            .map(|path| CachePin::open_until(path, deadline))
            .collect::<io::Result<Vec<_>>>()
            .map(|pins| pins.into_iter().flatten().collect())
    })
    .await
}

/// 清理调用位于业务锁之外，独立目录的失败分别完成后交回首个错误。
pub async fn prune_directories(paths: Vec<PathBuf>, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    blocking(timeout, move || {
        let mut failure = None;
        for path in paths {
            let result = lock_directory(&path, deadline).and_then(|_lock| prune_locked(&path));
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    })
    .await
}

async fn blocking<T: Send + 'static>(
    timeout: Duration,
    body: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::time::timeout(timeout, tokio::task::spawn_blocking(body))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "缓存文件操作超过截止时间"))?
        .map_err(io::Error::other)?
}

fn lock_directory(directory: &Path, deadline: Instant) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".sequence.lock"))?;
    lock_file(&file, false, deadline)?;
    Ok(file)
}

fn lock_file(file: &File, shared: bool, deadline: Instant) -> io::Result<()> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "库缓存文件锁等待超时",
            ));
        }
        match if shared {
            file.try_lock_shared()
        } else {
            file.try_lock()
        } {
            Ok(()) => return Ok(()),
            Err(TryLockError::Error(error)) => return Err(error),
            Err(TryLockError::WouldBlock) => std::thread::sleep(
                Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())),
            ),
        }
    }
}

fn marker_path(directory: &Path, sequence: u64) -> PathBuf {
    directory.join(format!(".copy-{sequence}.lock"))
}

/// SW-1：计数文件原子发布，锁文件保留固定 inode；首次运行接续已有库名。
fn next_sequence(directory: &Path) -> io::Result<u64> {
    let mut maximum = 0;
    for entry in fs::read_dir(directory)? {
        if let Some(sequence) = entry?.file_name().to_str().and_then(copy_sequence) {
            maximum = maximum.max(sequence);
        }
    }
    let last = match fs::read_to_string(directory.join(".sequence")) {
        Ok(value) => maximum.max(value.trim().parse::<u64>().map_err(io::Error::other)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => maximum,
        Err(error) => return Err(error),
    };
    let sequence = last
        .checked_add(1)
        .ok_or_else(|| io::Error::other("库副本序号已达到上限"))?;
    let staged = directory.join(".sequence.next");
    let mut file = File::create(&staged)?;
    writeln!(file, "{sequence}")?;
    file.sync_all()?;
    fs::rename(staged, directory.join(".sequence"))?;
    Ok(sequence)
}

fn copy_sequence(name: &str) -> Option<u64> {
    let (stem, extension) = name.rsplit_once('.')?;
    matches!(extension, "dylib" | "so").then_some(())?;
    stem.rsplit_once('-')?.1.parse().ok()
}

/// 标记与副本共享同一 inode，旧格式或外部来源的同序号文件保持独立。
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

/// 持有目录锁后按序号归集拥有标记和文件，一次扫描后在独占租约下删除。
fn prune_locked(directory: &Path) -> io::Result<usize> {
    let mut copies = BTreeMap::<u64, Vec<PathBuf>>::new();
    let mut owners = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(sequence) = name
            .strip_prefix(".copy-")
            .and_then(|name| name.strip_suffix(".lock"))
            .and_then(|name| name.parse::<u64>().ok())
        {
            owners.push((sequence, entry.path()));
        } else if let Some(sequence) = copy_sequence(name) {
            copies.entry(sequence).or_default().push(entry.path());
        }
    }
    let mut removed = 0;
    let mut failure = None;
    for (sequence, path) in owners {
        match prune_owner(&path, copies.remove(&sequence).unwrap_or_default()) {
            Ok(count) => removed += count,
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    failure.map_or(Ok(removed), Err)
}

fn prune_owner(path: &Path, copies: Vec<PathBuf>) -> io::Result<usize> {
    let owner = File::open(path)?;
    match owner.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(0),
        Err(TryLockError::Error(error)) => return Err(error),
    }
    let mut removed = 0;
    for copy in copies {
        if !same_file(&owner.metadata()?, &fs::metadata(&copy)?) {
            continue;
        }
        for suffix in [".verified.json", ".verified.json.pending"] {
            let mut path = copy.as_os_str().to_owned();
            path.push(suffix);
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        fs::remove_file(copy)?;
        removed += 1;
    }
    fs::remove_file(path)?;
    Ok(removed)
}
