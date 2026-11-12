use super::{Result, error};
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

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub enum LogOrigin {
    Protocol,
    Stderr,
}
#[derive(Clone, Debug, Serialize)]
pub struct LogRecord {
    pub origin: LogOrigin,
    pub connection: u64,
    pub context: u64,
    pub instance: String,
    pub level: u32,
    pub message: String,
}
pub type LogSink = std::sync::Arc<dyn Fn(LogRecord) + Send + Sync>;

#[derive(Clone, Debug, Serialize)]
pub struct GroupSnapshot {
    pub name: String,
    pub connection: Option<u64>,
    pub pid: Option<u32>,
    pub state: String,
    pub restarts: u32,
    pub plugins: Vec<payload::Plugin>,
    pub error: Option<Failure>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InstanceSnapshot {
    pub name: String,
    pub group: String,
    pub plugin: String,
    pub component: String,
    pub state: String,
    pub context: Option<u64>,
    pub revision: Option<u64>,
    pub bindings: Vec<u64>,
    pub error: Option<Failure>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReplaceReport {
    pub group: String,
    pub previous_connection: u64,
    pub candidate_connection: u64,
    pub affected_instances: usize,
    pub succeeded: bool,
    pub rolled_back: bool,
    pub failure: Option<Failure>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub groups: Vec<GroupSnapshot>,
    pub instances: Vec<InstanceSnapshot>,
    pub last_replace: Vec<ReplaceReport>,
}

#[derive(Clone, Debug)]
pub(super) struct Interface {
    pub full: String,
    pub key: String,
    pub minor: u32,
}

impl Interface {
    pub fn parse(full: &str) -> Result<Self> {
        let (name, version) = full
            .rsplit_once('@')
            .ok_or_else(|| error(2, "接口需要名称和版本"))?;
        if name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
            })
        {
            return Err(error(2, "接口名称需要小写字母、数字、短横线或点"));
        }
        let version = semver::Version::parse(&format!("{version}.0"))
            .map_err(|problem| error(2, problem.to_string()))?;
        let major =
            u32::try_from(version.major).map_err(|problem| error(2, problem.to_string()))?;
        let minor =
            u32::try_from(version.minor).map_err(|problem| error(2, problem.to_string()))?;
        Ok(Self {
            full: full.into(),
            key: format!("{name}@{major}"),
            minor,
        })
    }
}
