//! DOC-15 §3.2、CP-11：持久化部署与各实例的检查点。

use crate::config::{self, Overlay, Result, parameter};
use cordis_up::{Limits, Value, WireError, payload};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// DOC-16 §5：文件系统失败保留原始说明并作为通用协议错误交回调用方。
pub fn io_error(problem: std::io::Error) -> WireError {
    WireError::new(1, problem.to_string())
}

/// CP-2：读取配置或状态时以帧上限约束分配，超限在文件输入边界返回错误。
pub fn read(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io_error)?
        .take(Limits::DEFAULT.max_frame as u64 + 1)
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
    let staged = path.with_extension("next");
    let mut file = File::create(&staged).map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    if durable {
        file.sync_all().map_err(io_error)?;
    }
    fs::rename(staged, path).map_err(io_error)
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

impl Store {
    /// CP-11、PR-7：准备检查点消费目录和插件数据目录，并固定存储根路径。
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

    /// CP-11：优先使用最新检查点，其次使用上次启动消费的状态。
    pub fn load(&self, name: &str) -> Result<Option<payload::State>> {
        config::instance_name(name)?;
        for path in [
            self.directory.join(format!("{name}.state")),
            self.directory
                .join("consumed")
                .join(format!("{name}.state")),
        ] {
            match fs::metadata(&path) {
                Ok(_) => return parse_state(&read(&path)?).map(Some),
                Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {}
                Err(problem) => return Err(io_error(problem)),
            }
        }
        Ok(None)
    }

    /// DOC-11 §9：挂载接受状态后将文件转入 consumed，保留再次启动所需内容。
    pub fn consume(&self, name: &str) -> Result<()> {
        let path = self.directory.join(format!("{name}.state"));
        match fs::rename(
            &path,
            self.directory
                .join("consumed")
                .join(format!("{name}.state")),
        ) {
            Ok(()) => Ok(()),
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => Ok(()),
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

    /// DOC-11 §7、CP-11：发布替换继承状态，并移除旧消费记录以保持重启后的相同语义。
    pub fn restore_states(&self, states: &HashMap<String, Option<payload::State>>) -> Result<()> {
        for (name, state) in states {
            config::instance_name(name)?;
            if let Some(state) = state {
                let value = Value::serialized(state).map_err(parameter)?;
                let bytes = cordis_up::encode_value(&value, &Limits::DEFAULT).map_err(parameter)?;
                write(&self.directory.join(format!("{name}.state")), &bytes)?;
                remove_file(
                    &self
                        .directory
                        .join("consumed")
                        .join(format!("{name}.state")),
                )?;
            } else {
                self.forget(name)?;
            }
        }
        Ok(())
    }

    /// DOC-11 §5：删除已移除实例或失效组件身份的活动及消费检查点。
    pub fn forget(&self, name: &str) -> Result<()> {
        for path in [
            self.directory.join(format!("{name}.state")),
            self.directory
                .join("consumed")
                .join(format!("{name}.state")),
        ] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {}
                Err(problem) => return Err(io_error(problem)),
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
        fs::create_dir(store.directory.join(format!("{blocked}.next"))).unwrap();
        assert!(store.checkpoint(&states).is_err());
        assert_eq!(
            store.load(healthy).unwrap().unwrap().data,
            Value::from(37u64)
        );
        case.success();
    }
}
