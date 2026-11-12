use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryInfo {
    pub id: u64,
    pub name: String,
    pub version: String,
    pub sdk_version: String,
    pub build_info: String,
    pub sequence: u64,
    pub path: PathBuf,
    pub verified: bool,
    pub leases: usize,
    pub quiescent: bool,
    pub retired: bool,
    pub closed: bool,
    pub components: Vec<ComponentInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentInfo {
    pub name: String,
    pub inject: Vec<DependencyInfo>,
    pub provides: Vec<String>,
    pub listens: Vec<String>,
    pub state_schema: String,
    pub state_version_min: u32,
    pub state_version_max: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DependencyInfo {
    pub interface: String,
    pub required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub name: String,
    pub plugin: String,
    pub component: String,
    pub state: String,
    pub revision: Option<u64>,
    pub bindings: Vec<u64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwapReport {
    pub plugin: String,
    pub previous_library: u64,
    pub candidate_library: u64,
    pub affected_instances: usize,
    pub pause_ms: u64,
    pub quiescence_ms: Option<u64>,
    pub succeeded: bool,
    pub rolled_back: bool,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub libraries: Vec<LibraryInfo>,
    pub instances: Vec<InstanceInfo>,
    pub last_swap: Vec<SwapReport>,
}

/// ADR-0017：装载器维护的库、实例和替换报告快照。
#[crate::interface(name = "cordis.runtime", version = "1.0")]
pub trait Runtime: Send + Sync {
    fn snapshot(&self) -> Result<RuntimeSnapshot>;
}

/// ADR-0017：契约套件报告断言，并请求宿主协调状态往返。
#[crate::interface(name = "cordis.contract", version = "1.0")]
pub trait Contract: Send + Sync {
    fn report(&self, name: &str, passed: bool, message: &str) -> Result<()>;
    fn finish(&self) -> Result<()>;
    fn stage(&self) -> Result<u32>;
    fn request_state_roundtrip(&self) -> Result<()>;
}
