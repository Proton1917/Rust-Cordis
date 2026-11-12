#![forbid(unsafe_code)]
//! ADR-0028：运行时、宿主与 SDK 共用的接口数据，序列化形式由各接口版本定义。

mod context;
pub use context::{ContextOptions, Interception, Isolation};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub connection: Option<u64>,
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
pub struct InstanceInfo<F = String> {
    pub name: String,
    pub plugin: String,
    pub component: String,
    pub state: String,
    pub revision: Option<u64>,
    pub bindings: Vec<u64>,
    pub error: Option<F>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub reference: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwapReport {
    pub plugin: String,
    pub previous_library: u64,
    pub candidate_library: u64,
    pub affected_instances: usize,
    pub pause_ms: u64,
    pub quiescence_ms: Option<u64>,
    #[serde(default)]
    pub committed: bool,
    pub succeeded: bool,
    pub rolled_back: bool,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginInfo {
    pub reference: String,
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub components: Vec<ComponentInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupInfo<P = String, F = String> {
    pub name: String,
    pub connection: Option<u64>,
    pub pid: Option<u32>,
    pub state: String,
    pub restarts: u32,
    pub plugins: Vec<P>,
    pub error: Option<F>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupReplaceReport<F = String> {
    pub group: String,
    pub previous_connection: u64,
    pub candidate_connection: u64,
    pub affected_instances: usize,
    #[serde(default)]
    pub committed: bool,
    pub succeeded: bool,
    pub rolled_back: bool,
    pub failure: Option<F>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessSnapshot<P = String, F = String> {
    pub groups: Vec<GroupInfo<P, F>>,
    pub instances: Vec<InstanceInfo<F>>,
    pub last_replace: Vec<GroupReplaceReport<F>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogOrigin {
    Protocol,
    Stderr,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogRecord {
    pub origin: LogOrigin,
    pub connection: u64,
    pub context: u64,
    pub instance: String,
    pub level: u32,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchTicket {
    pub id: String,
}

/// ADR-0031：核心持久化的最近批次结果，由请求标识关联到调用方的待确认窗口。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchOutcome {
    pub id: String,
    pub caller: String,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub libraries: Vec<LibraryInfo>,
    pub instances: Vec<InstanceInfo>,
    pub last_swap: Vec<SwapReport>,
    #[serde(default)]
    pub available_plugins: Vec<PluginInfo>,
    #[serde(default)]
    pub groups: Vec<GroupInfo>,
    #[serde(default)]
    pub last_replace: Vec<GroupReplaceReport>,
    #[serde(default)]
    pub last_batch: Vec<BatchOutcome>,
}
