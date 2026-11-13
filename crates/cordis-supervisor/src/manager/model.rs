//! DOC-15 CP-4、CP-6：监管配置、部署记录、失败报告与接口版本的边界表示。

use crate::PeerOptions;
use cordis_up::{Value, payload};
use serde::Serialize;
use std::{path::PathBuf, time::Duration};

#[derive(Clone)]
pub struct GroupSpec {
    pub name: String,
    pub command: Vec<String>,
    pub directory: PathBuf,
    pub peer: PeerOptions,
    pub restart: RestartPolicy,
}

#[derive(Clone, Debug)]
pub struct RestartPolicy {
    pub max: u32,
    pub backoff: Duration,
}
impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max: 5,
            backoff: Duration::from_millis(500),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManagerOptions {
    pub operation_queue_capacity: usize,
    pub max_exports: usize,
    pub max_registrations_per_context: usize,
    pub max_retained_metadata_bytes: usize,
    pub handshake_timeout: Duration,
    pub apply_timeout: Duration,
    pub cleanup_timeout: Duration,
    pub export_timeout: Duration,
    pub invoke_timeout: Duration,
    pub swap_timeout: Duration,
    pub confirm: Duration,
    pub retired_image_limit: u64,
}

impl Default for ManagerOptions {
    fn default() -> Self {
        Self {
            operation_queue_capacity: 64,
            max_exports: 65_536,
            max_registrations_per_context: 65_536,
            max_retained_metadata_bytes: 64 * 1024 * 1024,
            handshake_timeout: Duration::from_secs(10),
            apply_timeout: Duration::from_secs(30),
            cleanup_timeout: Duration::from_secs(10),
            export_timeout: Duration::from_secs(5),
            invoke_timeout: Duration::from_secs(30),
            swap_timeout: Duration::from_secs(30),
            confirm: Duration::from_secs(10),
            retired_image_limit: 20,
        }
    }
}

#[derive(Clone, Debug)]
pub struct InstanceSpec {
    pub name: String,
    pub group: String,
    pub plugin: String,
    pub component: String,
    pub config: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct Failure {
    pub code: u32,
    pub message: String,
}
impl From<&cordis_up::WireError> for Failure {
    fn from(value: &cordis_up::WireError) -> Self {
        Self {
            code: value.code,
            message: super::metadata::clipped(&value.message),
        }
    }
}

pub use cordis_types::{LogOrigin, LogRecord};
pub type LogSink = std::sync::Arc<dyn Fn(LogRecord) + Send + Sync>;
pub type GroupSnapshot = cordis_types::GroupInfo<payload::Plugin, Failure>;
pub type InstanceSnapshot = cordis_types::InstanceInfo<Failure>;
pub type ReplaceReport = cordis_types::GroupReplaceReport<Failure>;
pub type Snapshot = cordis_types::ProcessSnapshot<payload::Plugin, Failure>;
