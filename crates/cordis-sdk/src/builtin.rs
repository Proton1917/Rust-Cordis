use crate::Result;
pub use cordis_types::{
    ComponentInfo, DependencyInfo, GroupInfo, GroupReplaceReport, InstanceInfo, LibraryInfo,
    PluginInfo, RuntimeSnapshot, SwapReport,
};
use serde::{Deserialize, Serialize};

/// PR-7：宿主为当前插件分配的持久化目录。
#[crate::interface(name = "cordis.data-dir", version = "1.0")]
pub trait DataDir: Send + Sync {
    fn path(&self) -> Result<String>;
}

/// ADR-0017：运行时派发的同步日志记录。
#[crate::event(name = "cordis.log", version = "1.0", kind = "sync")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogRecord {
    /// 0 Error，1 Warn，2 Info，3 Debug。
    pub level: u32,
    pub message: String,
    pub instance: String,
}

/// ADR-0017：装载器维护的库、实例和替换报告快照。
#[crate::interface(name = "cordis.runtime", version = "1.1")]
pub trait Runtime: Send + Sync {
    fn snapshot(&self) -> Result<RuntimeSnapshot>;
}

/// ADR-0024：核心按实例授权执行串行管理操作。
#[crate::interface(name = "cordis.manage", version = "1.0")]
pub trait Manage: Send + Sync {
    async fn install(&self, source: &str) -> Result<String>;
    async fn add(
        &self,
        name: &str,
        plugin: &str,
        group: Option<String>,
        config: crate::CborValue,
    ) -> Result<()>;
    async fn remove(&self, name: &str) -> Result<()>;
    async fn replace(&self, name: &str, plugin: &str) -> Result<()>;
    async fn rollback(&self, name: &str) -> Result<()>;
    async fn configure(&self, name: &str, config: crate::CborValue) -> Result<()>;
    async fn reload(&self) -> Result<()>;
    async fn shutdown(&self) -> Result<()>;
}

/// ADR-0017：契约套件报告断言，并请求宿主协调状态往返。
#[crate::interface(name = "cordis.contract", version = "1.0")]
pub trait Contract: Send + Sync {
    fn report(&self, name: &str, passed: bool, message: &str) -> Result<()>;
    fn finish(&self) -> Result<()>;
    fn stage(&self) -> Result<u32>;
    fn request_state_roundtrip(&self) -> Result<()>;
}
