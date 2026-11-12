use crate::{Context, Error, Interface, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EmptyConfig {}

/// DOC-11 §2：组件的依赖、接口、状态声明与加载行为。
pub trait Component: Send + Sync + 'static {
    const NAME: &'static str;
    type Config: DeserializeOwned + Send + Sync + 'static;

    fn inject() -> Inject {
        Inject::new()
    }
    fn provides() -> Provides {
        Provides::new()
    }
    fn listens() -> Listens {
        Listens::new()
    }
    fn state() -> StateSchema {
        StateSchema::default()
    }
    fn resolve(_config: &Self::Config) -> Result<()> {
        Ok(())
    }
    fn apply(ctx: Context, config: Self::Config) -> impl Future<Output = Result<()>> + Send;
}

#[derive(Clone, Debug)]
pub struct Dependency {
    pub interface: &'static str,
    pub required: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Inject(pub(crate) Vec<Dependency>);

impl Inject {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with<I: Interface + ?Sized>(mut self) -> Self {
        self.0.push(Dependency {
            interface: I::INTERFACE,
            required: true,
        });
        self
    }

    pub fn optional<I: Interface + ?Sized>(mut self) -> Self {
        self.0.push(Dependency {
            interface: I::INTERFACE,
            required: false,
        });
        self
    }

    pub fn entries(&self) -> &[Dependency] {
        &self.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct Provides(pub(crate) Vec<&'static str>);

impl Provides {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with<I: Interface + ?Sized>(mut self) -> Self {
        self.0.push(I::INTERFACE);
        self
    }

    pub fn entries(&self) -> &[&'static str] {
        &self.0
    }
}

pub trait Event: Serialize + DeserializeOwned + Send + Sync + 'static {
    const INTERFACE: &'static str;
    const MAJOR: u32;
    const MINOR: u32;
}

pub trait SyncEvent: Event {}
pub trait AsyncEvent: Event {}

#[derive(Clone, Debug, Default)]
pub struct Listens(pub(crate) Vec<&'static str>);

impl Listens {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with<E: Event>(mut self) -> Self {
        self.0.push(E::INTERFACE);
        self
    }

    pub fn entries(&self) -> &[&'static str] {
        &self.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct StateSchema {
    pub name: &'static str,
    pub min_version: u32,
    pub max_version: u32,
}

impl StateSchema {
    pub const fn new(name: &'static str, min_version: u32, max_version: u32) -> Self {
        Self {
            name,
            min_version,
            max_version,
        }
    }
}

/// DOC-11 §5.3：写路径在持有自身状态锁时读取冻结标志。
#[derive(Clone, Default)]
pub struct Frozen(Arc<AtomicBool>);

impl Frozen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn check(&self) -> Result<()> {
        if self.is_set() {
            Err(Error::new(4, "组件正在导出状态"))
        } else {
            Ok(())
        }
    }

    pub(crate) fn freeze(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub(crate) fn thaw(&self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
pub struct StateBlob {
    pub(crate) schema: String,
    pub(crate) version: u32,
    pub(crate) data: Vec<u8>,
}

impl StateBlob {
    pub fn schema(&self) -> &str {
        &self.schema
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn decode<T: DeserializeOwned>(&self) -> Result<T> {
        crate::decode(&self.data)
    }
}
