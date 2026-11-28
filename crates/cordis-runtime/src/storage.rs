//! DOC-15 §3.2、CP-11：持久化部署与各实例的检查点。

use crate::config::{self, Overlay, Result, parameter};
use cordis_up::{Limits, Value, WireError, payload};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const OWNER: &str = "runtime.pid";

/// 核心持有状态目录的写入权，进程退出时操作系统释放文件锁。
#[cfg(unix)]
pub(crate) fn claim(directory: &Path) -> Result<(File, File)> {
    fs::create_dir_all(directory).map_err(io_error)?;
    let writer = File::open(directory).map_err(io_error)?;
    match writer.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(parameter(format!(
                "状态目录已有运行中的核心：{}",
                directory.display()
            )));
        }
        Err(TryLockError::Error(error)) => return Err(io_error(error)),
    }
    // 先锁住本次启动的 PID 文件再原子发布，查询使用独立的文件描述符。
    let staged = directory.join("runtime.pid.next");
    let mut owner = File::create(&staged).map_err(io_error)?;
    owner.try_lock().map_err(|error| io_error(error.into()))?;
    write!(owner, "{}", std::process::id()).map_err(io_error)?;
    remove_file(&directory.join("status.json"))?;
    fs::rename(staged, directory.join(OWNER)).map_err(io_error)?;
    Ok((writer, owner))
}

#[cfg(unix)]
pub(crate) fn owner_pid(directory: &Path) -> Result<Option<u32>> {
    let mut file = match File::open(directory.join(OWNER)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(None),
        Err(TryLockError::WouldBlock) => {
            let mut pid = String::new();
            file.read_to_string(&mut pid).map_err(io_error)?;
            Ok(Some(pid.parse().map_err(parameter)?))
        }
        Err(TryLockError::Error(error)) => Err(io_error(error)),
    }
}

/// Windows 禁止打开目录句柄加锁且文件锁为强制锁，拆分目录锁与 PID 锁避免阻塞读者。
#[cfg(windows)]
pub(crate) fn claim(directory: &Path) -> Result<(File, File)> {
    fs::create_dir_all(directory).map_err(io_error)?;
    let writer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".runtime.lock"))
        .map_err(io_error)?;
    match writer.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(parameter(format!(
                "状态目录已有运行中的核心：{}",
                directory.display()
            )));
        }
        Err(TryLockError::Error(error)) => return Err(io_error(error)),
    }
    let owner = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("runtime.pid.lock"))
        .map_err(io_error)?;
    owner.try_lock().map_err(|error| io_error(error.into()))?;
    remove_file(&directory.join("status.json"))?;
    publish(
        &directory.join(OWNER),
        std::process::id().to_string().as_bytes(),
        false,
    )?;
    Ok((writer, owner))
}

#[cfg(windows)]
pub(crate) fn owner_pid(directory: &Path) -> Result<Option<u32>> {
    let lock = match File::open(directory.join("runtime.pid.lock")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    match lock.try_lock_shared() {
        Ok(()) => Ok(None),
        Err(TryLockError::WouldBlock) => {
            let mut file = match File::open(directory.join(OWNER)) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(io_error(error)),
            };
            let mut pid = String::new();
            file.read_to_string(&mut pid).map_err(io_error)?;
            Ok(Some(pid.trim().parse().map_err(parameter)?))
        }
        Err(TryLockError::Error(error)) => Err(io_error(error)),
    }
}

/// DOC-16 §5：文件系统失败保留原始说明并作为通用协议错误交回调用方。
pub fn io_error(problem: std::io::Error) -> WireError {
    WireError::new(1, problem.to_string())
}

/// CP-2：读取配置或状态时以帧上限约束分配，超限在文件输入边界返回错误。
pub fn read(path: &Path) -> Result<Vec<u8>> {
    read_file(File::open(path).map_err(io_error)?, path)
}

fn read_file(file: File, path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(Limits::DEFAULT.max_frame as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > Limits::DEFAULT.max_frame {
        return Err(parameter(format!("文件超过 16 MiB：{}", path.display())));
    }
    Ok(bytes)
}

/// CP-11：同步暂存文件后原子替换目标，失败保留文件系统错误。
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    publish(path, bytes, true)
}

/// CP-11：原子替换目标但不强制刷盘，用于随时可从实时状态重建的快照（如 `status.json`）。
///
/// 读者仍只会看到完整的旧文件或新文件；断电后最多丢失最近一次快照，核心启动时会重新生成。
pub fn write_snapshot(path: &Path, bytes: &[u8]) -> Result<()> {
    publish(path, bytes, false)
}

fn publish(path: &Path, bytes: &[u8], durable: bool) -> Result<()> {
    let staged = path.with_added_extension("next");
    let mut file = File::create(&staged).map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    if durable {
        file.sync_all().map_err(io_error)?;
    }
    drop(file);
    fs::rename(staged, path).map_err(io_error)
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(test)]
    sync_test::before(path)?;
    sync_directory_contents(path)
}

fn sync_directory_contents(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(io_error)
    }
    #[cfg(windows)]
    {
        let metadata = fs::metadata(path).map_err(io_error)?;
        if !metadata.is_dir() {
            return Err(io_error(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!("路径不是目录：{}", path.display()),
            )));
        }
        Ok(())
    }
}

/// Confirm the new directories and every name that makes the store reachable.
/// An earlier failed attempt may have left readable directories behind, so each
/// attempt repeats the complete sequence, including existing ancestors.
fn prepare_directory_tree(
    directory: &Path,
    mut sync: impl FnMut(&Path) -> Result<()>,
) -> Result<PathBuf> {
    fs::create_dir_all(directory.join("consumed")).map_err(io_error)?;
    fs::create_dir_all(directory.join("data")).map_err(io_error)?;
    let directory = directory.canonicalize().map_err(io_error)?;
    sync(&directory.join("consumed"))?;
    sync(&directory.join("data"))?;
    for ancestor in directory.ancestors() {
        sync(ancestor)?;
    }
    Ok(directory)
}

#[cfg(test)]
pub(crate) mod sync_test {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static CALLS: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
        static FAILURE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    pub(crate) fn fail_next(path: &Path) {
        FAILURE.with(|failure| *failure.borrow_mut() = Some(path.to_owned()));
    }

    pub(crate) fn calls(path: &Path) -> usize {
        CALLS.with(|calls| calls.borrow().iter().filter(|call| *call == path).count())
    }

    pub(super) fn before(path: &Path) -> Result<()> {
        CALLS.with(|calls| calls.borrow_mut().push(path.to_owned()));
        if FAILURE.with(|failure| failure.borrow().as_deref() == Some(path)) {
            FAILURE.with(|failure| failure.borrow_mut().take());
            return Err(io_error(std::io::Error::other(
                "injected directory sync failure",
            )));
        }
        Ok(())
    }
}

/// CP-8、CP-9：状态块使用 UP/1 受限 CBOR 解码器和共享 State 数据定义。
pub fn parse_state(bytes: &[u8]) -> Result<payload::State> {
    let value = cordis_up::decode_value(bytes, &Limits::DEFAULT).map_err(parameter)?;
    value.deserialized().map_err(parameter)
}

#[derive(Clone)]
pub struct Store {
    pub directory: PathBuf,
    checkpoints: Arc<Mutex<CheckpointCache>>,
}

const CHECKPOINT_CACHE_BYTES: usize = 16 * 1024 * 1024;
const CHECKPOINT_CACHE_ENTRIES: usize = 256;

struct PublishedCheckpoint {
    state: Option<Arc<payload::State>>,
    bytes: Arc<[u8]>,
    age: u64,
    charged: usize,
}

#[derive(Default)]
struct CheckpointCache {
    directory: PathBuf,
    entries: HashMap<String, PublishedCheckpoint>,
    order: BTreeMap<u64, String>,
    bytes: usize,
    next: u64,
    #[cfg(test)]
    encodes: usize,
    #[cfg(test)]
    reads: usize,
    #[cfg(test)]
    writes: usize,
}

impl CheckpointCache {
    fn bind(&mut self, directory: &Path) {
        if self.directory != directory {
            self.clear();
            self.directory = directory.to_owned();
        } else if !directory.is_absolute() {
            // The public Store path may be reassigned. A relative spelling can
            // resolve differently after a process cwd change, so it is not a key.
            self.clear();
        }
    }

    fn remove(&mut self, name: &str) {
        if let Some(old) = self.entries.remove(name) {
            self.order.remove(&old.age);
            self.bytes -= old.charged;
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    fn remember(&mut self, name: &str, state: Option<Arc<payload::State>>, bytes: Arc<[u8]>) {
        self.remove(name);
        // Charge every retained allocation even when the State Arc is shared.
        // Index/Arc bookkeeping is charged conservatively; allocator fragmentation
        // and process RSS remain independent measurements.
        let charged = bytes
            .len()
            .saturating_add(512)
            .saturating_add(name.len().saturating_mul(2))
            .saturating_add(state.as_ref().map_or(0, |state| {
                std::mem::size_of::<payload::State>()
                    .saturating_add(128)
                    .saturating_add(state.schema.capacity())
                    .saturating_add(value_heap(&state.data))
            }));
        if charged > CHECKPOINT_CACHE_BYTES {
            return;
        }
        while self.entries.len() >= CHECKPOINT_CACHE_ENTRIES
            || self.bytes.saturating_add(charged) > CHECKPOINT_CACHE_BYTES
        {
            let (_, oldest) = self.order.first_key_value().expect("cache has an entry");
            let oldest = oldest.clone();
            self.remove(&oldest);
        }
        if self.next == u64::MAX {
            self.clear();
            self.next = 0;
        }
        let age = self.next;
        self.next += 1;
        self.bytes += charged;
        self.order.insert(age, name.to_owned());
        self.entries.insert(
            name.to_owned(),
            PublishedCheckpoint {
                state,
                bytes,
                age,
                charged,
            },
        );
    }
}

fn value_heap(value: &Value) -> usize {
    const ALLOCATION: usize = 64;
    match value {
        Value::Text(value) => value.capacity().saturating_add(ALLOCATION),
        Value::Bytes(value) => value.capacity().saturating_add(ALLOCATION),
        Value::Array(values) => values.iter().fold(
            values
                .capacity()
                .saturating_mul(std::mem::size_of::<Value>())
                .saturating_add(ALLOCATION),
            |total, value| total.saturating_add(value_heap(value)),
        ),
        Value::Map(values) => values.iter().fold(
            values
                .capacity()
                .saturating_mul(std::mem::size_of::<(Value, Value)>())
                .saturating_add(ALLOCATION),
            |total, (key, value)| {
                total
                    .saturating_add(value_heap(key))
                    .saturating_add(value_heap(value))
            },
        ),
        Value::Tag(_, value) => std::mem::size_of::<Value>()
            .saturating_add(ALLOCATION)
            .saturating_add(value_heap(value)),
        _ => 0,
    }
}

// Equality used to skip encoding preserves signed zero and floating bit patterns.
#[cfg(test)]
thread_local! {
    static VALUE_COMPARISONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn same_value(left: &Value, right: &Value) -> bool {
    #[cfg(test)]
    VALUE_COMPARISONS.with(|count| count.set(count.get() + 1));
    match (left, right) {
        (Value::Float(left), Value::Float(right)) => left.to_bits() == right.to_bits(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(a, b)| same_value(a, b))
        }
        (Value::Map(left), Value::Map(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|((ak, av), (bk, bv))| same_value(ak, bk) && same_value(av, bv))
        }
        (Value::Tag(a, av), Value::Tag(b, bv)) => a == b && same_value(av, bv),
        _ => left == right,
    }
}

fn same_state(left: &payload::State, right: &payload::State) -> bool {
    if std::ptr::eq(left, right) {
        return true;
    }
    left.schema == right.schema
        && left.version == right.version
        && same_value(&left.data, &right.data)
}

#[derive(Debug)]
pub(crate) enum CommitError {
    NotCommitted(WireError),
    RecoveryRequired(WireError),
}

impl From<WireError> for CommitError {
    fn from(error: WireError) -> Self {
        Self::NotCommitted(error)
    }
}

/// 决定已同步到磁盘，清理错误随已提交结果返回。
#[derive(Debug)]
pub(crate) struct Committed {
    pub cleanup_error: Option<WireError>,
}

/// All fallible serialization and size checks finish before the undo journal is published.
struct PreparedDeployment<'a> {
    base: String,
    overlay: String,
    states: HashMap<String, Option<Vec<u8>>>,
    sources: &'a [u8],
}

impl<'a> PreparedDeployment<'a> {
    fn encode<S: std::borrow::Borrow<payload::State>>(
        base: &config::Configuration,
        overlay: &Overlay,
        states: &HashMap<String, Option<S>>,
        sources: &'a [u8],
    ) -> Result<Self> {
        let base = toml::to_string(base).map_err(parameter)?;
        config::text(base.as_bytes())?;
        let overlay = encode_overlay(overlay)?;
        if sources.len() > Limits::DEFAULT.max_frame {
            return Err(parameter("来源记录超过 16 MiB"));
        }
        let states = states
            .iter()
            .map(|(name, state)| {
                config::instance_name(name)?;
                let bytes = state
                    .as_ref()
                    .map(|state| {
                        cordis_up::encode_state(state.borrow(), &Limits::DEFAULT).map_err(parameter)
                    })
                    .transpose()?;
                Ok((name.clone(), bytes))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            base,
            overlay,
            states,
            sources,
        })
    }
}

fn encode_overlay(overlay: &Overlay) -> Result<String> {
    let bytes = toml::to_string_pretty(overlay).map_err(parameter)?;
    config::text(bytes.as_bytes())?;
    Ok(bytes)
}

const BASE: &str = "deployment-base-v1.toml";
const JOURNAL_FORMAT: &str = ".format-version";
const JOURNAL_VERSION: &[u8] = b"2\n";

// consumed 是部署目录中位置固定的目录。进程全程持有的所有者锁位于其父目录。
fn deployment_lock_file(directory: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        File::open(directory.join("consumed"))
    }
    #[cfg(windows)]
    {
        let consumed = directory.join("consumed");
        if !consumed.is_dir() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(consumed.join(".deployment.lock"))
    }
}

pub(crate) fn read_committed_sources(path: &Path) -> Result<Vec<u8>> {
    let directory = path
        .parent()
        .ok_or_else(|| parameter("来源记录缺少状态目录"))?;
    let lock = match deployment_lock_file(directory) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // 旧归档可单独保留来源文件；并发首次启动时改从受锁保护的部署读取。
            let archived = File::open(path);
            match deployment_lock_file(directory) {
                Ok(lock) => lock,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return read_file(archived.map_err(io_error)?, path);
                }
                Err(error) => return Err(io_error(error)),
            }
        }
        Err(error) => return Err(io_error(error)),
    };
    lock.lock_shared().map_err(io_error)?;
    let journal = directory.join(".deployment-undo");
    if journal.try_exists().map_err(io_error)? {
        check_journal_format(&journal)?;
        read(&journal.join("sources.json"))
    } else {
        read(path)
    }
}

fn check_journal_format(journal: &Path) -> Result<()> {
    let path = journal.join(JOURNAL_FORMAT);
    match File::open(&path) {
        Ok(file) => {
            if read_file(file, &path)? == JOURNAL_VERSION {
                Ok(())
            } else {
                Err(parameter("部署日志使用未知格式版本"))
            }
        }
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(problem) => Err(io_error(problem)),
    }
}

impl Store {
    fn lock_deployment(&self) -> Result<File> {
        let lock = deployment_lock_file(&self.directory).map_err(io_error)?;
        lock.lock().map_err(io_error)?;
        Ok(lock)
    }

    /// 持有运行目录写锁时，把中断部署触及的文件恢复到提交前的版本。
    pub(crate) fn recover_deployment(&self) -> Result<()> {
        let _lock = self.lock_deployment()?;
        self.checkpoints.lock().clear();
        self.restore_deployment_files()?;
        if let Err(problem) = self.clear_staging() {
            eprintln!("已恢复部署，暂存目录清理失败：{problem}");
        }
        // 旧版可能同时留下两个版本，根目录里的检查点较新。
        let legacy = self.directory.join("consumed");
        let mut migrated = Vec::new();
        for entry in fs::read_dir(&legacy).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "state")
                && entry.file_type().map_err(io_error)?.is_file()
            {
                let target = self.directory.join(entry.file_name());
                match fs::hard_link(&path, &target) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        if !fs::symlink_metadata(target).map_err(io_error)?.is_file() {
                            continue;
                        }
                    }
                    Err(e) => return Err(io_error(e)),
                }
                migrated.push(path);
            }
        }
        if !migrated.is_empty() {
            sync_directory(&self.directory)?;
            for path in migrated {
                fs::remove_file(path).map_err(io_error)?;
            }
            sync_directory(&legacy)?;
        }
        Ok(())
    }

    fn restore_deployment_files(&self) -> Result<()> {
        self.checkpoints.lock().clear();
        let journal = self.directory.join(".deployment-undo");
        if journal.try_exists().map_err(io_error)? {
            check_journal_format(&journal)?;
            for folder in ["", "consumed"] {
                let destination = self.directory.join(folder);
                let entries = match fs::read_dir(journal.join(folder)) {
                    Ok(entries) => entries,
                    Err(e) if folder == "consumed" && e.kind() == std::io::ErrorKind::NotFound => {
                        continue;
                    }
                    Err(e) => return Err(io_error(e)),
                };
                for file in entries {
                    let file = file.map_err(io_error)?;
                    if file.file_type().map_err(io_error)?.is_dir() {
                        continue;
                    }
                    let name = file
                        .file_name()
                        .into_string()
                        .map_err(|_| parameter("部署记录文件名无效"))?;
                    if folder.is_empty() && name == JOURNAL_FORMAT {
                        continue;
                    }
                    if let Some(absent) = name.strip_suffix(".absent") {
                        remove_file(&destination.join(absent))?;
                    } else {
                        let target = destination.join(name);
                        let link = target.with_added_extension("recovering");
                        remove_file(&link)?;
                        fs::hard_link(file.path(), &link).map_err(io_error)?;
                        fs::rename(&link, target).map_err(io_error)?;
                        remove_file(&link)?;
                    }
                }
                sync_directory(&destination)?;
            }
            self.move_decision_marker()?;
            sync_directory(&self.directory)?;
        }
        Ok(())
    }

    fn clear_staging(&self) -> Result<()> {
        let staging = self.directory.join(".deployment-staging");
        match fs::remove_dir_all(staging) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error(e)),
        }
    }

    fn move_decision_marker(&self) -> Result<()> {
        fs::rename(
            self.directory.join(".deployment-undo"),
            self.directory.join(".deployment-staging"),
        )
        .map_err(io_error)
    }

    /// 用原文件的硬链接保存回退点。覆盖层、代码来源和实例状态共享一次提交。
    fn prepare_deployment<'a>(&self, names: impl Iterator<Item = &'a String>) -> Result<()> {
        self.restore_deployment_files()?;
        self.clear_staging()?;
        let staging = self.directory.join(".deployment-staging");
        fs::create_dir(&staging).map_err(io_error)?;
        write(&staging.join(JOURNAL_FORMAT), JOURNAL_VERSION)?;
        let preserve = |relative: &Path| -> Result<()> {
            let saved = staging.join(relative);
            match fs::hard_link(self.directory.join(relative), &saved) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    File::create(saved.with_added_extension("absent")).map_err(io_error)?;
                    Ok(())
                }
                Err(error) => Err(io_error(error)),
            }
        };
        preserve(Path::new("instances.toml"))?;
        preserve(Path::new("sources.json"))?;
        preserve(Path::new(BASE))?;
        for name in names {
            config::instance_name(name)?;
            preserve(Path::new(&format!("{name}.state")))?;
        }
        sync_directory(&staging)?;
        fs::rename(&staging, self.directory.join(".deployment-undo")).map_err(io_error)?;
        sync_directory(&self.directory)
    }

    pub(crate) fn commit_deployment<S: std::borrow::Borrow<payload::State>>(
        &self,
        base: &config::Configuration,
        overlay: &Overlay,
        states: &HashMap<String, Option<S>>,
        sources: &[u8],
    ) -> std::result::Result<Committed, CommitError> {
        let prepared = PreparedDeployment::encode(base, overlay, states, sources)?;
        let _lock = self.lock_deployment()?;
        if let Err(problem) = self.prepare_deployment(prepared.states.keys()) {
            return Err(self.reject_deployment(problem));
        }
        self.commit_prepared(&prepared)
    }

    fn commit_prepared(
        &self,
        prepared: &PreparedDeployment<'_>,
    ) -> std::result::Result<Committed, CommitError> {
        self.checkpoints.lock().clear();
        let result = (|| {
            write(&self.directory.join(BASE), prepared.base.as_bytes())?;
            for (name, bytes) in &prepared.states {
                let path = self.directory.join(format!("{name}.state"));
                match bytes {
                    Some(bytes) => write(&path, bytes)?,
                    None => remove_file(&path)?,
                }
            }
            write(
                &self.directory.join("instances.toml"),
                prepared.overlay.as_bytes(),
            )?;
            write(&self.directory.join("sources.json"), prepared.sources)?;
            sync_directory(&self.directory)?;
            self.move_decision_marker()
        })();
        if let Err(problem) = result {
            return Err(self.reject_deployment(problem));
        }
        self.sync_decision()
    }

    fn reject_deployment(&self, problem: WireError) -> CommitError {
        match self.restore_deployment_files() {
            Ok(()) => CommitError::NotCommitted(problem),
            Err(recovery) => CommitError::RecoveryRequired(WireError::new(
                1,
                format!("{problem}；部署恢复失败：{recovery}"),
            )),
        }
    }

    fn sync_decision(&self) -> std::result::Result<Committed, CommitError> {
        sync_directory(&self.directory).map_err(CommitError::RecoveryRequired)?;
        Ok(Committed {
            cleanup_error: self.clear_staging().err(),
        })
    }

    pub(crate) fn accepted_base(&self) -> Result<Option<config::Configuration>> {
        let path = self.directory.join(BASE);
        match File::open(&path) {
            Ok(file) => config::parse(&read_file(file, &path)?).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_error(e)),
        }
    }

    /// CP-11、PR-7：持久确认部署锁目录、插件数据目录和祖先路径，固定存储根路径。
    pub fn create(directory: PathBuf) -> Result<Self> {
        let directory = prepare_directory_tree(&directory, sync_directory_contents)?;
        Ok(Self {
            directory,
            checkpoints: Arc::new(Mutex::new(CheckpointCache::default())),
        })
    }

    /// DOC-15 §3.2：首次启动使用空叠加层，已有叠加层按配置契约解析。
    pub fn overlay(&self) -> Result<Overlay> {
        let path = self.directory.join("instances.toml");
        match fs::metadata(&path) {
            Ok(_) => config::parse_overlay(&read(&path)?),
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
                Ok(Overlay::default())
            }
            Err(problem) => Err(io_error(problem)),
        }
    }

    /// DOC-15 §3.2：序列化已提交部署并原子发布 instances.toml。
    pub fn save_overlay(&self, overlay: &Overlay) -> Result<()> {
        let bytes = encode_overlay(overlay)?;
        self.publish_record("instances.toml", bytes.as_bytes())
    }

    /// ADR-0054：来源确认与离线读取使用同一写入边界，目录同步后才确认成功。
    pub(crate) fn save_sources(&self, bytes: &[u8]) -> Result<()> {
        self.publish_record("sources.json", bytes)
    }

    fn publish_record(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let _lock = self.lock_deployment()?;
        write(&self.directory.join(name), bytes)?;
        sync_directory(&self.directory)
    }

    /// CP-11：实例重启时读取最近一次保存的检查点。
    pub fn load(&self, name: &str) -> Result<Option<payload::State>> {
        config::instance_name(name)?;
        let path = self.directory.join(format!("{name}.state"));
        match File::open(&path) {
            Ok(file) => parse_state(&read_file(file, &path)?).map(Some),
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(problem) => Err(io_error(problem)),
        }
    }

    /// CP-11：发布状态并复用最后成功发布的不可变编码。
    /// 缓存属于持有状态目录写权的 Store；克隆共享缓存，部署恢复使其失效。
    pub fn checkpoint<S: std::borrow::Borrow<payload::State>>(
        &self,
        states: &HashMap<String, S>,
    ) -> Result<()> {
        self.checkpoint_entries(
            states
                .iter()
                .map(|(name, state)| (name, state.borrow(), None)),
        )
    }

    /// Runtime 已持有 Arc<State>，内容未变时复用原编码及持久发布事实。
    pub fn checkpoint_shared(&self, states: &HashMap<String, Arc<payload::State>>) -> Result<()> {
        self.checkpoint_entries(
            states
                .iter()
                .map(|(name, state)| (name, state.as_ref(), Some(state))),
        )
    }

    fn checkpoint_entries<'a>(
        &self,
        states: impl Iterator<
            Item = (
                &'a String,
                &'a payload::State,
                Option<&'a Arc<payload::State>>,
            ),
        >,
    ) -> Result<()> {
        let _lock = self.lock_deployment()?;
        let mut cache = self.checkpoints.lock();
        cache.bind(&self.directory);
        let mut failure = None;
        let mut needs_sync = false;
        for (name, state, shared) in states {
            let result: Result<()> = (|| {
                config::instance_name(name)?;
                if let Some(published) = cache.entries.get(name)
                    && let Some(previous) = &published.state
                    && same_state(previous, state)
                {
                    return Ok(());
                }
                #[cfg(test)]
                {
                    cache.encodes += 1;
                }
                let bytes: Arc<[u8]> = cordis_up::encode_state(state, &Limits::DEFAULT)
                    .map_err(parameter)?
                    .into();
                let path = self.directory.join(format!("{name}.state"));
                let published = cache.entries.contains_key(name);
                let unchanged = match cache.entries.get(name) {
                    Some(previous) => previous.bytes == bytes,
                    None => {
                        #[cfg(test)]
                        {
                            cache.reads += 1;
                        }
                        read(&path).is_ok_and(|current| current.as_slice() == bytes.as_ref())
                    }
                };
                if !unchanged {
                    cache.remove(name);
                    #[cfg(test)]
                    {
                        cache.writes += 1;
                    }
                    write(&path, &bytes)?;
                    needs_sync = true;
                } else if !published {
                    // A readable file can come from a publication whose directory
                    // sync failed. Confirm both its contents and name once before
                    // making it eligible for the warm-cache fast path.
                    #[cfg(unix)]
                    let synced = File::open(&path).and_then(|file| file.sync_all());
                    #[cfg(windows)]
                    let synced = fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .and_then(|file| file.sync_all());
                    synced.map_err(io_error)?;
                    needs_sync = true;
                }
                // The cache lock keeps this batch private until directory sync.
                cache.remember(name, shared.cloned(), bytes);
                Ok(())
            })();
            if let Err(problem) = result {
                failure.get_or_insert_with(|| {
                    WireError::new(
                        problem.code,
                        format!("实例 {name} 检查点保存失败：{}", problem.message),
                    )
                });
            }
        }
        if needs_sync && let Err(problem) = sync_directory(&self.directory) {
            cache.clear();
            failure.get_or_insert(problem);
        }
        failure.map_or(Ok(()), Err)
    }

    /// DOC-11 §7、CP-11：发布替换继承状态，清除已移除实例或失效身份的检查点。
    pub fn restore_states<S: std::borrow::Borrow<payload::State>>(
        &self,
        states: &HashMap<String, Option<S>>,
    ) -> Result<()> {
        let _lock = self.lock_deployment()?;
        self.checkpoints.lock().clear();
        for (name, state) in states {
            config::instance_name(name)?;
            if let Some(state) = state {
                let bytes =
                    cordis_up::encode_state(state.borrow(), &Limits::DEFAULT).map_err(parameter)?;
                write(&self.directory.join(format!("{name}.state")), &bytes)?;
            } else {
                remove_file(&self.directory.join(format!("{name}.state")))?;
            }
        }
        if !states.is_empty() {
            sync_directory(&self.directory)?;
        }
        Ok(())
    }

    /// PR-7：按逻辑插件名称分配数据目录，名称在路径边界核验。
    pub fn data_dir(&self, plugin: &str) -> Result<PathBuf> {
        config::instance_name(plugin)?;
        let directory = self.directory.join("data").join(plugin);
        fs::create_dir_all(&directory).map_err(io_error)?;
        Ok(directory)
    }
}

/// DOC-15 §3.2：清除被取代的状态文件，首次状态缺席保持幂等。
fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(problem) => Err(io_error(problem)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_testkit::cleanup::TestGuard;

    fn snapshot(value: Value) -> Arc<payload::State> {
        Arc::new(payload::State {
            schema: "counter".into(),
            version: 1,
            data: value,
        })
    }

    #[test]
    fn directory_initialization_confirms_children_before_ancestors() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "directory-initialization-order",
        );
        let directory = case.directory("initialization").join("nested/state");
        let mut calls = Vec::new();
        let canonical = prepare_directory_tree(&directory, |path| {
            calls.push(path.to_owned());
            sync_directory_contents(path)
        })
        .unwrap();
        let expected: Vec<_> = [canonical.join("consumed"), canonical.join("data")]
            .into_iter()
            .chain(canonical.ancestors().map(Path::to_owned))
            .collect();
        assert_eq!(calls, expected);
        assert!(canonical.is_absolute());
        assert!(canonical.join("consumed").is_dir());
        assert!(canonical.join("data").is_dir());
        case.success();
    }

    #[test]
    fn directory_initialization_retries_every_barrier_after_failure() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "directory-initialization-retry",
        );
        let directory = case.directory("initialization").join("nested/state");
        fs::create_dir_all(&directory).unwrap();
        let canonical = directory.canonicalize().unwrap();
        let expected: Vec<_> = [canonical.join("consumed"), canonical.join("data")]
            .into_iter()
            .chain(canonical.ancestors().map(Path::to_owned))
            .collect();
        for failed in 0..expected.len() {
            let mut attempted = Vec::new();
            let result = prepare_directory_tree(&directory, |path| {
                attempted.push(path.to_owned());
                if attempted.len() == failed + 1 {
                    return Err(io_error(std::io::Error::other("initialization barrier")));
                }
                sync_directory_contents(path)
            });
            assert!(result.is_err());
            assert_eq!(attempted, expected[..=failed]);
            let mut retried = Vec::new();
            prepare_directory_tree(&directory, |path| {
                retried.push(path.to_owned());
                sync_directory_contents(path)
            })
            .unwrap();
            assert_eq!(retried, expected);
        }
        case.success();
    }

    #[test]
    fn checkpoint_shared_reuses_encoding_and_published_bytes() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "checkpoint-cache");
        let store = Store::create(case.directory("checkpoint-cache")).unwrap();
        let original = snapshot(Value::Bytes(vec![17; 1024 * 1024]));
        let states = HashMap::from([("panel".into(), original.clone())]);
        store.checkpoint_shared(&states).unwrap();
        let clone = store.clone();
        // An independently exported but unchanged value also reuses the encoding.
        let repeated = HashMap::from([("panel".into(), snapshot(original.data.clone()))]);
        clone.checkpoint_shared(&repeated).unwrap();
        {
            let cache = store.checkpoints.lock();
            assert_eq!((cache.encodes, cache.reads, cache.writes), (1, 1, 1));
        }
        let next = HashMap::from([("panel".into(), snapshot(42u64.into()))]);
        clone.checkpoint_shared(&next).unwrap();
        let cache = store.checkpoints.lock();
        assert_eq!((cache.encodes, cache.reads, cache.writes), (2, 1, 2));
        drop(cache);
        assert_eq!(
            store.load("panel").unwrap().unwrap().data,
            Value::from(42u64)
        );
        case.success();
    }

    #[test]
    fn checkpoint_shared_identity_skips_value_scans() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "checkpoint-shared-identity",
        );
        let store = Store::create(case.directory("checkpoint-shared-identity")).unwrap();
        let original = snapshot(Value::Array(
            (0..256)
                .map(|index| Value::Bytes(vec![index as u8; 4096]))
                .collect(),
        ));
        let states = HashMap::from([("panel".into(), original.clone())]);
        store.checkpoint_shared(&states).unwrap();
        let comparisons = VALUE_COMPARISONS.with(std::cell::Cell::get);
        for _ in 0..1_000 {
            store.checkpoint_shared(&states).unwrap();
        }
        assert_eq!(VALUE_COMPARISONS.with(std::cell::Cell::get), comparisons);
        let distinct = HashMap::from([("panel".into(), snapshot(original.data.clone()))]);
        store.checkpoint_shared(&distinct).unwrap();
        assert_eq!(
            VALUE_COMPARISONS.with(std::cell::Cell::get),
            comparisons + 257
        );
        let cache = store.checkpoints.lock();
        assert_eq!((cache.encodes, cache.reads, cache.writes), (1, 1, 1));
        drop(cache);
        case.success();
    }

    #[test]
    fn checkpoint_cache_follows_each_stores_directory() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "checkpoint-directory-owner",
        );
        let first = Store::create(case.directory("checkpoint-directory-first")).unwrap();
        let second = Store::create(case.directory("checkpoint-directory-second")).unwrap();
        let mut moved = first.clone();
        let one = HashMap::from([("panel".into(), snapshot(1u64.into()))]);
        first.checkpoint_shared(&one).unwrap();
        moved.directory = second.directory;
        moved.checkpoint_shared(&one).unwrap();
        assert_eq!(moved.load("panel").unwrap().unwrap().data, 1u64.into());
        let two = HashMap::from([("panel".into(), snapshot(2u64.into()))]);
        moved.checkpoint_shared(&two).unwrap();
        first.checkpoint_shared(&two).unwrap();
        assert_eq!(first.load("panel").unwrap().unwrap().data, 2u64.into());
        moved.checkpoint_shared(&two).unwrap();
        assert_eq!(moved.load("panel").unwrap().unwrap().data, 2u64.into());

        // Relative Store paths remain supported; a reused spelling is not proof
        // of the same directory when the process can change its working directory.
        let mut cache = CheckpointCache::default();
        cache.bind(Path::new("state"));
        cache.remember("panel", None, Arc::from([0u8]));
        cache.bind(Path::new("state"));
        assert!(cache.entries.is_empty());
        case.success();
    }

    #[test]
    fn checkpoint_batch_confirms_directory_before_reusing_published_bytes() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "checkpoint-directory-sync",
        );
        let store = Store::create(case.directory("checkpoint-directory-sync")).unwrap();
        let states = HashMap::from([
            ("first".into(), snapshot(1u64.into())),
            ("second".into(), snapshot(2u64.into())),
        ]);
        sync_test::fail_next(&store.directory);
        let failure = store.checkpoint_shared(&states).unwrap_err();
        assert!(failure.message.contains("directory sync failure"));
        // Real file writes and renames completed before the injected barrier failure.
        assert_eq!(store.load("first").unwrap().unwrap().data, 1u64.into());
        assert_eq!(store.load("second").unwrap().unwrap().data, 2u64.into());
        assert!(store.checkpoints.lock().entries.is_empty());
        assert_eq!(sync_test::calls(&store.directory), 1);

        store.clone().checkpoint_shared(&states).unwrap();
        assert_eq!(sync_test::calls(&store.directory), 2);
        {
            let cache = store.checkpoints.lock();
            assert_eq!(cache.entries.len(), 2);
            assert_eq!((cache.encodes, cache.reads, cache.writes), (4, 4, 2));
        }
        store.checkpoint_shared(&states).unwrap();
        assert_eq!(sync_test::calls(&store.directory), 2);
        let cache = store.checkpoints.lock();
        assert_eq!((cache.encodes, cache.reads, cache.writes), (4, 4, 2));
        drop(cache);
        case.success();
    }

    #[test]
    fn ordinary_records_and_state_restoration_require_directory_confirmation() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "record-directory-sync",
        );
        let store = Store::create(case.directory("record-directory-sync")).unwrap();
        sync_test::fail_next(&store.directory);
        assert!(store.save_overlay(&Overlay::default()).is_err());
        sync_test::fail_next(&store.directory);
        assert!(store.save_sources(b"{}").is_err());
        let states = HashMap::from([("panel".into(), Some(snapshot(1u64.into())))]);
        sync_test::fail_next(&store.directory);
        assert!(store.restore_states(&states).is_err());
        assert_eq!(sync_test::calls(&store.directory), 3);
        store.restore_states(&states).unwrap();
        assert_eq!(sync_test::calls(&store.directory), 4);
        case.success();
    }

    #[test]
    fn checkpoint_cache_updates_only_after_success_and_invalidates_on_restore() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "checkpoint-cache-retry",
        );
        let store = Store::create(case.directory("checkpoint-cache-retry")).unwrap();
        let old = snapshot(1u64.into());
        let new = snapshot(2u64.into());
        store
            .checkpoint_shared(&HashMap::from([("panel".into(), old.clone())]))
            .unwrap();
        let blocked = store.directory.join("panel.state.next");
        fs::create_dir(&blocked).unwrap();
        let changed = HashMap::from([("panel".into(), new.clone())]);
        assert!(store.checkpoint_shared(&changed).is_err());
        assert!(!store.checkpoints.lock().entries.contains_key("panel"));
        fs::remove_dir(&blocked).unwrap();
        store.checkpoint_shared(&changed).unwrap();
        assert_eq!(store.load("panel").unwrap().unwrap().data, new.data);
        store
            .restore_states(&HashMap::from([("panel".into(), Some(old.clone()))]))
            .unwrap();
        store.checkpoint_shared(&changed).unwrap();
        assert_eq!(store.load("panel").unwrap().unwrap().data, new.data);
        // A committed deployment is another writer under the same Store owner.
        store
            .commit_deployment(
                &config::parse(b"").unwrap(),
                &Overlay::default(),
                &HashMap::from([("panel".into(), Some(old))]),
                b"{}",
            )
            .unwrap();
        store.checkpoint_shared(&changed).unwrap();
        assert_eq!(store.load("panel").unwrap().unwrap().data, new.data);
        case.success();
    }

    #[test]
    fn checkpoint_cache_preserves_signed_zero_and_bounds_encoded_retention() {
        let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "checkpoint-zero");
        let store = Store::create(case.directory("checkpoint-zero")).unwrap();
        for value in [0.0f64, -0.0f64] {
            store
                .checkpoint_shared(&HashMap::from([(
                    "panel".into(),
                    snapshot(Value::Float(value)),
                )]))
                .unwrap();
            let loaded = store.load("panel").unwrap().unwrap();
            assert_eq!(loaded.data.as_float().unwrap().to_bits(), value.to_bits());
        }
        let mut cache = store.checkpoints.lock();
        for n in 0..300 {
            cache.remember(&format!("entry-{n}"), None, Arc::from([0u8]));
        }
        assert!(cache.entries.len() <= CHECKPOINT_CACHE_ENTRIES);
        cache.remember(
            "large",
            None,
            Arc::from(vec![0u8; CHECKPOINT_CACHE_BYTES / 2]),
        );
        assert!(cache.entries.contains_key("large"));
        assert!(cache.bytes <= CHECKPOINT_CACHE_BYTES);
        let mut sparse =
            Vec::with_capacity(CHECKPOINT_CACHE_BYTES / std::mem::size_of::<Value>() + 1);
        sparse.push(Value::Null);
        cache.remember(
            "tiny-encoding-large-capacity",
            Some(snapshot(Value::Array(sparse))),
            Arc::from([0u8]),
        );
        assert!(!cache.entries.contains_key("tiny-encoding-large-capacity"));
        assert!(cache.bytes <= CHECKPOINT_CACHE_BYTES);
        drop(cache);
        case.success();
    }

    #[test]
    fn previous_configuration_is_read_compatibly_and_persisted_as_program_identity() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "compact-previous");
        let store = Store::create(case.directory("compact-previous")).unwrap();
        let base = config::parse(b"[instance.panel]\nplugin='installed-1'\n").unwrap();
        let legacy = config::parse_overlay(
            b"[previous.panel]\nplugin='installed-1'\ngroup='old-group'\n[previous.panel.config]\nvalue=42\n",
        ).unwrap();
        assert_eq!(
            legacy.previous["panel"].config["value"].as_integer(),
            Some(42)
        );
        let mut settings = base.instance["panel"].clone();
        settings.config.insert(
            "blob".into(),
            toml::Value::String("x".repeat(9 * 1024 * 1024)),
        );
        let mut overlay = Overlay::default();
        overlay
            .previous
            .insert("panel".into(), config::OverlayEntry::from(&settings));
        settings.plugin = "installed-2".into();
        overlay
            .instance
            .insert("panel".into(), config::OverlayEntry::from(&settings));
        store
            .commit_deployment::<payload::State>(&base, &overlay, &HashMap::new(), b"{}")
            .unwrap();
        let recovered = store.overlay().unwrap();
        assert_eq!(recovered.instance["panel"].config, settings.config);
        assert_eq!(recovered.previous["panel"].plugin, "installed-1");
        assert!(recovered.previous["panel"].config.is_empty());
        assert!(
            fs::metadata(store.directory.join("instances.toml"))
                .unwrap()
                .len()
                < Limits::DEFAULT.max_frame as u64
        );
        case.success();
    }

    #[test]
    fn deployment_preflight_rejects_unreadable_files_before_any_journal_or_write() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "deployment-preflight",
        );
        let store = Store::create(case.directory("deployment-preflight")).unwrap();
        let base = config::parse(b"[instance.panel]\nplugin='installed-1'\n").unwrap();
        let overlay = Overlay::default();
        store
            .commit_deployment::<payload::State>(&base, &overlay, &HashMap::new(), b"old")
            .unwrap();
        let old_base = read(&store.directory.join(BASE)).unwrap();
        let old_overlay = read(&store.directory.join("instances.toml")).unwrap();
        let oversized = vec![b'x'; Limits::DEFAULT.max_frame + 1];
        let sources =
            store.commit_deployment::<payload::State>(&base, &overlay, &HashMap::new(), &oversized);
        assert!(matches!(sources, Err(CommitError::NotCommitted(_))));
        let mut settings = base.instance["panel"].clone();
        settings.config.insert(
            "blob".into(),
            toml::Value::String(String::from_utf8(oversized).unwrap()),
        );
        let mut oversized_overlay = Overlay::default();
        oversized_overlay
            .instance
            .insert("panel".into(), config::OverlayEntry::from(&settings));
        let active = store.commit_deployment::<payload::State>(
            &base,
            &oversized_overlay,
            &HashMap::new(),
            b"new",
        );
        assert!(matches!(active, Err(CommitError::NotCommitted(_))));
        let invalid_states = HashMap::from([("../bad".into(), None::<payload::State>)]);
        assert!(matches!(
            store.commit_deployment(&base, &overlay, &invalid_states, b"new"),
            Err(CommitError::NotCommitted(_))
        ));
        assert_eq!(read(&store.directory.join(BASE)).unwrap(), old_base);
        assert_eq!(
            read(&store.directory.join("instances.toml")).unwrap(),
            old_overlay
        );
        assert_eq!(read(&store.directory.join("sources.json")).unwrap(), b"old");
        assert!(!store.directory.join(".deployment-undo").exists());
        assert!(!store.directory.join(".deployment-staging").exists());
        case.success();
    }

    #[test]
    fn write_and_marker_faults_preserve_the_previously_committed_deployment() {
        let case = TestGuard::for_package(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "decision-write-marker",
        );
        let store = Store::create(case.directory("decision-write-marker")).unwrap();
        let old_base = config::parse(b"[instance.old]\nplugin='kv'\n").unwrap();
        let new_base = b"[instance.new]\nplugin='kv'\n";
        let overlay = Overlay::default();
        let states = HashMap::<String, Option<payload::State>>::new();
        store
            .commit_deployment(&old_base, &overlay, &states, b"old")
            .unwrap();

        let blocked_file = store.directory.join(BASE).with_added_extension("next");
        fs::create_dir(&blocked_file).unwrap();
        let failure =
            store.commit_deployment(&config::parse(new_base).unwrap(), &overlay, &states, b"new");
        assert!(
            matches!(failure, Err(CommitError::NotCommitted(_))),
            "{failure:?}"
        );
        fs::remove_dir(blocked_file).unwrap();
        assert_eq!(
            read_committed_sources(&store.directory.join("sources.json")).unwrap(),
            b"old"
        );

        let staging = store.directory.join(".deployment-staging");
        {
            let _lock = store.lock_deployment().unwrap();
            store.prepare_deployment(states.keys()).unwrap();
            fs::create_dir(&staging).unwrap();
            File::create(staging.join("reserved")).unwrap();
            let prepared = PreparedDeployment::encode(
                &config::parse(new_base).unwrap(),
                &overlay,
                &states,
                b"new",
            )
            .unwrap();
            let failure = store.commit_prepared(&prepared);
            assert!(
                matches!(failure, Err(CommitError::RecoveryRequired(_))),
                "{failure:?}"
            );
        }
        assert_eq!(
            read_committed_sources(&store.directory.join("sources.json")).unwrap(),
            b"old"
        );
        fs::remove_file(staging.join("reserved")).unwrap();
        fs::remove_dir(staging).unwrap();
        let reopened = Store::create(store.directory.clone()).unwrap();
        reopened.recover_deployment().unwrap();
        assert_eq!(reopened.accepted_base().unwrap(), Some(old_base));
        assert_eq!(
            read_committed_sources(&reopened.directory.join("sources.json")).unwrap(),
            b"old"
        );
        case.success();
    }

    #[test]
    fn cleanup_error_preserves_the_committed_source_and_base() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "decision-cleanup");
        let store = Store::create(case.directory("decision-cleanup")).unwrap();
        let base = config::parse(b"[instance.old]\nplugin='kv'\n").unwrap();
        store
            .commit_deployment::<payload::State>(
                &base,
                &Overlay::default(),
                &HashMap::new(),
                b"old",
            )
            .unwrap();
        store.prepare_deployment(std::iter::empty()).unwrap();
        write(&store.directory.join("sources.json"), b"new").unwrap();
        write(
            &store.directory.join(BASE),
            b"[instance.new]\nplugin='kv'\n",
        )
        .unwrap();
        sync_directory(&store.directory).unwrap();
        store.move_decision_marker().unwrap();
        let staging = store.directory.join(".deployment-staging");
        let displaced = store.directory.join("saved-staging");
        fs::rename(&staging, &displaced).unwrap();
        File::create(&staging).unwrap();
        let result = store.sync_decision();
        assert!(result.unwrap().cleanup_error.is_some());
        assert_eq!(
            read_committed_sources(&store.directory.join("sources.json")).unwrap(),
            b"new"
        );
        fs::remove_file(&staging).unwrap();
        fs::rename(displaced, &staging).unwrap();
        store.recover_deployment().unwrap();
        assert!(
            store
                .accepted_base()
                .unwrap()
                .unwrap()
                .instance
                .contains_key("new")
        );
        case.success();
    }

    #[test]
    fn directory_sync_error_after_marker_move_requires_disk_recovery() {
        let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "decision-sync");
        let store = Store::create(case.directory("decision-sync").join("state")).unwrap();
        store.prepare_deployment(std::iter::empty()).unwrap();
        store.move_decision_marker().unwrap();
        let moved = store.directory.with_extension("detached");
        fs::rename(&store.directory, &moved).unwrap();
        assert!(matches!(
            store.sync_decision(),
            Err(CommitError::RecoveryRequired(_))
        ));
        fs::rename(moved, &store.directory).unwrap();
        store.recover_deployment().unwrap();
        case.success();
    }

    #[test]
    fn interrupted_deployment_restores_code_and_consumed_state_together() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "deployment-recovery");
        let dir = case.directory("deployment-recovery");
        let store = Store::create(dir.clone()).unwrap();
        let state = |version| payload::State {
            schema: "counter".into(),
            version,
            data: Value::from(37u64),
        };
        let before = HashMap::from([("store".into(), Some(state(1)))]);
        let after = HashMap::from([
            ("store".into(), Some(state(2))),
            ("second".into(), Some(state(2))),
        ]);
        let old = config::parse_overlay(b"[instance.store]\nplugin='one'\n").unwrap();
        let new = config::parse_overlay(b"[instance.store]\nplugin='two'\n").unwrap();
        let sources_v1 = br#"{"store":{"declared_source":"counter","installed_reference":"one","plugin_directory":"plugins"}}"#;
        let sources_v2 = br#"{"store":{"declared_source":"counter","installed_reference":"two","plugin_directory":"plugins"}}"#;
        let base = config::parse(b"").unwrap();
        store
            .commit_deployment(&base, &old, &before, sources_v1)
            .unwrap();
        store.prepare_deployment(after.keys()).unwrap();
        // v0.2.10 的回退日志可以只在 consumed 中保存原值。
        let journal = store.directory.join(".deployment-undo");
        fs::remove_file(journal.join(JOURNAL_FORMAT)).unwrap();
        fs::create_dir(journal.join("consumed")).unwrap();
        fs::rename(
            journal.join("store.state"),
            journal.join("consumed/store.state"),
        )
        .unwrap();
        File::create(journal.join("store.state.absent")).unwrap();
        store.restore_states(&after).unwrap();
        store.save_overlay(&new).unwrap();
        write(&store.directory.join("sources.json"), sources_v2).unwrap();
        let offline =
            crate::control::offline_sources(&store.directory.join("sources.json"), &dir).unwrap();
        assert_eq!(
            serde_json::to_value(offline).unwrap()["instances"]["store"]["installed_reference"],
            "one"
        );
        drop(store);
        let recovered = Store::create(dir).unwrap();
        // 查询在候选文件已发布时进入；写方恢复旧记录、移走日志后才释放锁。
        std::thread::scope(|scope| {
            let lock = recovered.lock_deployment().unwrap();
            let (started, ready) = std::sync::mpsc::channel();
            let (done, result) = std::sync::mpsc::channel();
            let directory = &recovered.directory;
            scope.spawn(move || {
                started.send(()).unwrap();
                let snapshot =
                    crate::control::offline_sources(&directory.join("sources.json"), directory)
                        .unwrap();
                done.send(serde_json::to_value(snapshot).unwrap()).unwrap();
            });
            ready.recv().unwrap();
            let waiting = result.recv_timeout(std::time::Duration::from_millis(100));
            recovered.restore_deployment_files().unwrap();
            drop(lock);
            assert!(matches!(
                waiting,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            assert_eq!(
                result.recv().unwrap()["instances"]["store"]["installed_reference"],
                "one"
            );
        });
        assert_eq!(recovered.overlay().unwrap().instance["store"].plugin, "one");
        assert_eq!(
            read(&recovered.directory.join("sources.json")).unwrap(),
            sources_v1
        );
        assert!(recovered.directory.join("consumed/store.state").is_file());
        recovered.recover_deployment().unwrap();
        assert_eq!(recovered.load("store").unwrap().unwrap().version, 1);
        assert!(!recovered.directory.join("consumed/store.state").exists());
        assert!(recovered.load("second").unwrap().is_none());
        recovered
            .commit_deployment(&base, &new, &after, sources_v2)
            .unwrap();
        recovered.recover_deployment().unwrap();
        assert_eq!(recovered.overlay().unwrap().instance["store"].plugin, "two");
        assert_eq!(
            read(&recovered.directory.join("sources.json")).unwrap(),
            sources_v2
        );
        assert_eq!(recovered.load("second").unwrap().unwrap().version, 2);
        case.success();
    }

    #[test]
    fn status_snapshot_and_status_instance_use_independent_staging() {
        let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "status-instance");
        let store = Store::create(case.directory("status-instance")).unwrap();
        fs::create_dir(store.directory.join("status.json.next")).unwrap();
        assert!(write_snapshot(&store.directory.join("status.json"), b"{}").is_err());
        let state = payload::State {
            schema: "counter".into(),
            version: 1,
            data: 43u64.into(),
        };
        store
            .checkpoint(&HashMap::from([("status".into(), state)]))
            .unwrap();
        assert_eq!(
            store.load("status").unwrap().unwrap().data,
            Value::from(43u64)
        );
        case.success();
    }

    #[test]
    fn checkpoint_preserves_other_instances_after_file_error() {
        let case =
            TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "checkpoint-files");
        let store = Store::create(case.directory("checkpoint-files")).unwrap();
        let states: HashMap<_, _> = ["first", "second"]
            .into_iter()
            .map(|name| {
                (
                    name.to_owned(),
                    payload::State {
                        schema: "counter".into(),
                        version: 1,
                        data: 37u64.into(),
                    },
                )
            })
            .collect();
        let mut names = states.keys();
        let blocked = names.next().unwrap();
        let healthy = names.next().unwrap();
        fs::create_dir(store.directory.join(format!("{blocked}.state.next"))).unwrap();
        assert!(store.checkpoint(&states).is_err());
        assert_eq!(
            store.load(healthy).unwrap().unwrap().data,
            Value::from(37u64)
        );
        assert_eq!(sync_test::calls(&store.directory), 1);
        let cache = store.checkpoints.lock();
        assert!(cache.entries.contains_key(healthy));
        assert!(!cache.entries.contains_key(blocked));
        drop(cache);
        case.success();
    }
}
