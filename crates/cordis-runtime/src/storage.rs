//! DOC-15 §3.2、CP-11：持久化部署与各实例的检查点。

use crate::config::{self, Overlay, Result, parameter};
use cordis_up::{Limits, Value, WireError, payload};
use std::{
    collections::HashMap,
    fs::{self, File, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const OWNER: &str = "runtime.pid";

/// 核心持有状态目录的写入权，进程退出时操作系统释放文件锁。
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
    fs::rename(staged, path).map_err(io_error)
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

/// CP-8、CP-9：状态块使用 UP/1 受限 CBOR 解码器和共享 State 数据定义。
pub fn parse_state(bytes: &[u8]) -> Result<payload::State> {
    let value = cordis_up::decode_value(bytes, &Limits::DEFAULT).map_err(parameter)?;
    value.deserialized().map_err(parameter)
}

#[derive(Clone)]
pub struct Store {
    pub directory: PathBuf,
}

// consumed 是部署目录中位置固定的目录。进程全程持有的所有者锁位于其父目录。
fn deployment_lock_file(directory: &Path) -> std::io::Result<File> {
    File::open(directory.join("consumed"))
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
        read(&journal.join("sources.json"))
    } else {
        read(path)
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
        self.restore_deployment_files()?;
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
        let journal = self.directory.join(".deployment-undo");
        if journal.try_exists().map_err(io_error)? {
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
            self.finish_deployment()?;
        }
        let staging = self.directory.join(".deployment-staging");
        if staging.try_exists().map_err(io_error)? {
            fs::remove_dir_all(staging).map_err(io_error)?;
        }
        Ok(())
    }

    fn finish_deployment(&self) -> Result<()> {
        fs::rename(
            self.directory.join(".deployment-undo"),
            self.directory.join(".deployment-staging"),
        )
        .map_err(io_error)?;
        sync_directory(&self.directory)
    }

    /// 用原文件的硬链接保存回退点。覆盖层、代码来源和实例状态共享一次提交。
    fn prepare_deployment<'a>(&self, names: impl Iterator<Item = &'a String>) -> Result<()> {
        self.restore_deployment_files()?;
        let staging = self.directory.join(".deployment-staging");
        fs::create_dir(&staging).map_err(io_error)?;
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
        for name in names {
            config::instance_name(name)?;
            preserve(Path::new(&format!("{name}.state")))?;
        }
        sync_directory(&staging)?;
        fs::rename(&staging, self.directory.join(".deployment-undo")).map_err(io_error)?;
        sync_directory(&self.directory)
    }

    pub(crate) fn commit_deployment(
        &self,
        overlay: &Overlay,
        states: &HashMap<String, Option<payload::State>>,
        sources: &[u8],
    ) -> Result<()> {
        let _lock = self.lock_deployment()?;
        self.prepare_deployment(states.keys())?;
        let result = (|| {
            self.restore_states(states)?;
            self.save_overlay(overlay)?;
            write(&self.directory.join("sources.json"), sources)?;
            sync_directory(&self.directory)?;
            self.finish_deployment()
        })();
        if let Err(problem) = result {
            self.restore_deployment_files().map_err(|recovery| {
                WireError::new(1, format!("{problem}；部署恢复失败：{recovery}"))
            })?;
            return Err(problem);
        }
        fs::remove_dir_all(self.directory.join(".deployment-staging")).map_err(io_error)
    }

    /// CP-11、PR-7：准备部署锁目录和插件数据目录，并固定存储根路径。
    pub fn create(directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(directory.join("consumed")).map_err(io_error)?;
        fs::create_dir_all(directory.join("data")).map_err(io_error)?;
        let directory = directory.canonicalize().map_err(io_error)?;
        Ok(Self { directory })
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
        let bytes = toml::to_string_pretty(overlay).map_err(parameter)?;
        write(&self.directory.join("instances.toml"), bytes.as_bytes())
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

    /// CP-11：逐实例发布真实导出状态，写入失败交由运行时记录。
    ///
    /// 已发布文件与本次导出逐字节相同时跳过写入：检查点周期可以很短（例如 1 秒），
    /// 每次都 `sync_all` 会让空闲实例持续刷盘。
    pub fn checkpoint(&self, states: &HashMap<String, payload::State>) -> Result<()> {
        let mut failure = None;
        for (name, state) in states {
            let result = (|| {
                config::instance_name(name)?;
                let value = Value::serialized(state).map_err(parameter)?;
                let bytes = cordis_up::encode_value(&value, &Limits::DEFAULT).map_err(parameter)?;
                let path = self.directory.join(format!("{name}.state"));
                if fs::read(&path).is_ok_and(|current| current == bytes) {
                    return Ok(());
                }
                write(&path, &bytes)
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
        failure.map_or(Ok(()), Err)
    }

    /// DOC-11 §7、CP-11：发布替换继承状态，清除已移除实例或失效身份的检查点。
    pub fn restore_states(&self, states: &HashMap<String, Option<payload::State>>) -> Result<()> {
        for (name, state) in states {
            config::instance_name(name)?;
            if let Some(state) = state {
                let value = Value::serialized(state).map_err(parameter)?;
                let bytes = cordis_up::encode_value(&value, &Limits::DEFAULT).map_err(parameter)?;
                write(&self.directory.join(format!("{name}.state")), &bytes)?;
            } else {
                remove_file(&self.directory.join(format!("{name}.state")))?;
            }
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
        store.commit_deployment(&old, &before, sources_v1).unwrap();
        store.prepare_deployment(after.keys()).unwrap();
        // v0.2.10 的回退日志可以只在 consumed 中保存原值。
        let journal = store.directory.join(".deployment-undo");
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
            .commit_deployment(&new, &after, sources_v2)
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
        case.success();
    }
}
