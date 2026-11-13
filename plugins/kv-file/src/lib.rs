//! DOC-11 §5、PR-6–PR-7：kv@1 的文件持久化、冻结导出与状态继承。

use cordis_sdk::builtin::DataDir;
use cordis_sdk::prelude::*;
use kv_api::{Changed, Kv, Snapshot};
use std::io::Write;
use std::path::{Component as PathComponent, Path, PathBuf};
use std::sync::Mutex;

#[derive(serde::Deserialize)]
pub struct FileConfig {
    pub path: String,
}

struct FileStore {
    context: Context,
    path: PathBuf,
    snapshot: Mutex<Snapshot>,
    frozen: Frozen,
}

impl FileStore {
    /// kv@1 flush：锁内取得一致快照，完整写入后原子发布，文件错误沿接口返回。
    fn flush_file(&self) -> Result<()> {
        let snapshot = self.snapshot.lock().unwrap();
        let bytes = cordis_sdk::encode(&*snapshot)?;
        let mut temporary = self.path.as_os_str().to_owned();
        temporary.push(".pending");
        let temporary = PathBuf::from(temporary);
        let mut file = std::fs::File::create(&temporary).map_err(io_error)?;
        file.write_all(&bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        std::fs::rename(&temporary, &self.path).map_err(io_error)
    }
}

#[cordis_sdk::interface]
impl Kv for FileStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .snapshot
            .lock()
            .unwrap()
            .entries
            .get(key)
            .map(|bytes| bytes.0.clone()))
    }
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let mut snapshot = self.snapshot.lock().unwrap();
        self.frozen.check()?;
        snapshot.entries.insert(key.to_owned(), value.into());
        drop(snapshot);
        self.context.emit(Changed {
            key: key.to_owned(),
        })
    }
    fn keys(&self) -> Result<Vec<String>> {
        Ok(self
            .snapshot
            .lock()
            .unwrap()
            .entries
            .keys()
            .cloned()
            .collect())
    }
    /// PR-6：内存合并先前写入，flush 将当前快照持久化至插件数据目录。
    async fn flush(&self) -> Result<()> {
        self.flush_file()
    }
}

fn io_error(problem: std::io::Error) -> Error {
    Error::new(1, problem.to_string())
}

pub struct KvStore;
impl Component for KvStore {
    const NAME: &'static str = "kv-store";
    type Config = FileConfig;
    fn inject() -> Inject {
        Inject::new().with::<dyn DataDir>()
    }
    fn provides() -> Provides {
        Provides::new().with::<dyn Kv>()
    }
    fn state() -> StateSchema {
        StateSchema::new("kv.snapshot", 1, 1)
    }
    /// PR-7：配置限定为数据目录内的相对文件路径，路径契约错误在加载前返回。
    fn resolve(config: &FileConfig) -> Result<()> {
        let path = Path::new(&config.path);
        if path.as_os_str().is_empty()
            || path.components().any(|component| {
                matches!(
                    component,
                    PathComponent::ParentDir | PathComponent::RootDir | PathComponent::Prefix(_)
                )
            })
        {
            return Err(Error::new(2, "path需要为插件数据目录内的相对文件名"));
        }
        Ok(())
    }
    /// DOC-11 §5、SW-5：优先继承导出状态，登记最终持久化与冻结导出后提供服务。
    async fn apply(ctx: Context, config: FileConfig) -> Result<()> {
        let path = ctx.data_dir()?.join(config.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let snapshot = match ctx.inherited_state()? {
            Some(blob) if blob.schema() == "kv.snapshot" && blob.version() == 1 => {
                blob.decode::<Snapshot>()?
            }
            Some(_) => return Err(Error::new(2, "kv-store需要kv.snapshot版本1")),
            None => match std::fs::read(&path) {
                Ok(bytes) => cordis_sdk::decode(&bytes)?,
                Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
                    Snapshot::default()
                }
                Err(problem) => return Err(io_error(problem)),
            },
        };
        let frozen = Frozen::new();
        let store = Arc::new(FileStore {
            context: ctx.clone(),
            path,
            snapshot: Mutex::new(snapshot),
            frozen: frozen.clone(),
        });
        ctx.effect("persist snapshot", {
            let store = store.clone();
            move |scope| scope.defer(move || store.flush_file())
        })?;
        ctx.export_state_with("kv.snapshot", 1, frozen, {
            let store = store.clone();
            move || Ok(store.snapshot.lock().unwrap().clone())
        })?;
        ctx.provide::<dyn Kv>(store)
    }
}

cordis_sdk::export_plugin!(
    name = "kv",
    version = env!("CARGO_PKG_VERSION"),
    components = [KvStore]
);
