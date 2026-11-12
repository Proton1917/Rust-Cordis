//! ADR-0028、DOC-15 CP-11：把真实内核与宿主状态发布为只读接口和状态文件。

use crate::{
    catalog::Package,
    config::{InstanceSettings, Result, parameter},
    storage,
};
use cordis_supervisor::manager::{
    GroupSnapshot, InstanceSnapshot, Manager, ReplaceReport, Snapshot,
};
use cordis_types::{
    BatchOutcome, ComponentInfo, DependencyInfo, GroupInfo, GroupReplaceReport, InstanceInfo,
    LibraryInfo, PluginInfo, RuntimeSnapshot, SwapReport,
};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Weak},
};

#[derive(Clone, Default, Serialize)]
pub struct Diagnostics {
    pub phase: String,
    pub configuration_error: Option<String>,
    pub instance_errors: BTreeMap<String, String>,
    pub plugin_errors: BTreeMap<String, String>,
    pub overrides: Vec<String>,
    pub sources: Vec<Package>,
    #[serde(skip)]
    pub desired: BTreeMap<String, InstanceSettings>,
    #[serde(skip)]
    pub active_references: BTreeMap<String, String>,
    #[serde(skip)]
    pub source_bindings: Option<Arc<Mutex<crate::control::SourceBindings>>>,
    pub last_batch: Vec<BatchOutcome>,
}

#[derive(Clone)]
pub struct View {
    pub manager: Weak<Manager>,
    pub diagnostics: Arc<Mutex<Diagnostics>>,
    ids: Arc<Mutex<LibraryIds>>,
}

#[derive(Default)]
struct LibraryIds {
    next: u64,
    known: BTreeMap<(u64, u64), u64>,
}
impl LibraryIds {
    fn get(&mut self, connection: u64, local: u64) -> u64 {
        *self.known.entry((connection, local)).or_insert_with(|| {
            self.next += 1;
            self.next
        })
    }
}

#[derive(Serialize)]
pub struct Status {
    pub core_version: &'static str,
    pub pid: u32,
    #[serde(flatten)]
    pub snapshot: Snapshot,
    #[serde(flatten)]
    pub diagnostics: Diagnostics,
    pub libraries: Vec<LibraryInfo>,
    pub last_swap: Vec<SwapReport>,
    pub available_plugins: Vec<PluginInfo>,
}

impl View {
    /// ADR-0028：状态视图持有管理器弱引用，发布任务随核心生命周期结束。
    pub fn new(manager: &Arc<Manager>, diagnostics: Arc<Mutex<Diagnostics>>) -> Self {
        Self {
            manager: Arc::downgrade(manager),
            diagnostics,
            ids: Arc::new(Mutex::new(LibraryIds::default())),
        }
    }

    /// ADR-0028：内核失败记录转换为接口诊断文本，其余字段保持真实来源。
    pub async fn runtime_snapshot(&self) -> Result<RuntimeSnapshot> {
        let observed = self.snapshot().await?;
        Ok(RuntimeSnapshot {
            libraries: observed.libraries,
            instances: observed
                .snapshot
                .instances
                .into_iter()
                .map(runtime_instance)
                .collect(),
            last_swap: observed.last_swap,
            available_plugins: observed.available_plugins,
            groups: observed
                .snapshot
                .groups
                .into_iter()
                .map(runtime_group)
                .collect(),
            last_replace: observed
                .snapshot
                .last_replace
                .into_iter()
                .map(runtime_replace)
                .collect(),
            last_batch: observed.diagnostics.last_batch,
        })
    }

    /// DOC-15 CP-5：锁外查询宿主，连接内库编号映射为运行时编号后发布快照。
    pub async fn snapshot(&self) -> Result<Status> {
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| cordis_up::WireError::new(4, "核心已退出"))?;
        let mut libraries = manager.library_snapshots().await?;
        libraries.sort_by_key(|library| (library.connection, library.id));
        let last_swap = self.map_library_ids(&manager, &mut libraries)?;
        let mut snapshot = manager.snapshot();
        let diagnostics = self.diagnostics.lock().clone();
        if let Some(sources) = &diagnostics.source_bindings {
            sources.lock().confirm(&snapshot)?;
        }
        complete_instances(&mut snapshot, &diagnostics);
        Ok(Status {
            core_version: env!("CARGO_PKG_VERSION"),
            pid: std::process::id(),
            snapshot,
            libraries,
            last_swap,
            available_plugins: diagnostics.sources.iter().map(package_info).collect(),
            diagnostics,
        })
    }

    /// ADR-0028：同一连接和本地库编号共享运行时编号，报告沿用相同映射。
    fn map_library_ids(
        &self,
        manager: &Manager,
        libraries: &mut [LibraryInfo],
    ) -> Result<Vec<SwapReport>> {
        let mut last_swap = Vec::new();
        {
            let mut ids = self.ids.lock();
            for library in libraries {
                let connection = library
                    .connection
                    .ok_or_else(|| cordis_up::WireError::new(4, "宿主库报告缺少连接编号"))?;
                library.id = ids.get(connection, library.id);
            }
            for (connection, mut report) in manager.library_swap_reports() {
                report.previous_library = ids.get(connection, report.previous_library);
                report.candidate_library = ids.get(connection, report.candidate_library);
                last_swap.push(report);
            }
        }
        Ok(last_swap)
    }
}

/// DOC-11 §10、ADR-0033：补充配置中保留的停用、等待和失败条目。
fn complete_instances(snapshot: &mut Snapshot, diagnostics: &Diagnostics) {
    for (name, settings) in &diagnostics.desired {
        if let Some(instance) = snapshot
            .instances
            .iter_mut()
            .find(|instance| instance.name == *name)
        {
            instance.reference = diagnostics.active_references.get(name).cloned();
            continue;
        }
        let failure = diagnostics.instance_errors.get(name);
        snapshot
            .instances
            .push(cordis_supervisor::manager::InstanceSnapshot {
                name: name.clone(),
                group: settings
                    .group
                    .clone()
                    .unwrap_or_else(|| settings.plugin.clone()),
                plugin: settings.plugin.clone(),
                component: settings.component.clone().unwrap_or_default(),
                state: if settings.disabled {
                    "disabled"
                } else if failure.is_some() {
                    "failed"
                } else {
                    "pending"
                }
                .into(),
                context: None,
                reference: Some(settings.plugin.clone()),
                revision: None,
                bindings: Vec::new(),
                error: failure.map(|message| cordis_supervisor::manager::Failure {
                    code: 1,
                    message: message.clone(),
                }),
            });
    }
    snapshot
        .instances
        .sort_by(|left, right| left.name.cmp(&right.name));
}

fn runtime_instance(instance: InstanceSnapshot) -> InstanceInfo {
    InstanceInfo {
        name: instance.name,
        plugin: instance.plugin,
        component: instance.component,
        state: instance.state,
        revision: instance.revision,
        bindings: instance.bindings,
        error: instance.error.map(|failure| failure.message),
        group: instance.group,
        context: instance.context,
        reference: instance.reference,
    }
}

fn runtime_group(group: GroupSnapshot) -> GroupInfo {
    GroupInfo {
        name: group.name,
        connection: group.connection,
        pid: group.pid,
        state: group.state,
        restarts: group.restarts,
        plugins: group
            .plugins
            .into_iter()
            .map(|plugin| plugin.name)
            .collect(),
        error: group.error.map(|failure| failure.message),
    }
}

fn runtime_replace(report: ReplaceReport) -> GroupReplaceReport {
    GroupReplaceReport {
        group: report.group,
        previous_connection: report.previous_connection,
        candidate_connection: report.candidate_connection,
        affected_instances: report.affected_instances,
        pause_ms: report.pause_ms,
        committed: report.committed,
        succeeded: report.succeeded,
        rolled_back: report.rolled_back,
        failure: report.failure.map(|failure| failure.message),
    }
}

fn package_info(package: &Package) -> PluginInfo {
    PluginInfo {
        reference: package.reference.clone(),
        name: package.name.clone(),
        version: package.version.clone(),
        path: package.source.clone(),
        components: package
            .descriptor
            .as_ref()
            .map(|descriptor| descriptor.components.iter().map(component).collect())
            .unwrap_or_default(),
    }
}

fn component(component: &cordis_up::payload::Component) -> ComponentInfo {
    ComponentInfo {
        name: component.name.clone(),
        inject: component
            .inject
            .iter()
            .map(|dependency| DependencyInfo {
                interface: dependency.interface.clone(),
                required: dependency.required,
            })
            .collect(),
        provides: component.provides.clone(),
        listens: component.listens.clone(),
        state_schema: component.state_schema.clone(),
        state_version_min: component.state_version_min,
        state_version_max: component.state_version_max,
    }
}

/// DOC-15 CP-11：快照变化时通过状态存储原子发布，错误由运行时结束路径处理。
pub async fn write(path: &Path, view: &View, previous: &mut Vec<u8>) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(&view.snapshot().await?).map_err(parameter)?;
    if *previous != bytes {
        storage::write(path, &bytes)?;
        *previous = bytes;
    }
    Ok(())
}
