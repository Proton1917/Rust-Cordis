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
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
    sync::{Arc, Weak},
};

#[derive(Clone, Default, Serialize)]
pub struct Diagnostics {
    #[serde(skip)]
    pub web: Option<Weak<cordis_web_deployment::Deployment>>,
    pub phase: String,
    pub configuration_error: Option<String>,
    pub recovery_error: Option<String>,
    pub instance_errors: BTreeMap<String, String>,
    pub plugin_errors: BTreeMap<String, String>,
    pub overrides: Vec<String>,
    pub sources: Vec<Package>,
    #[serde(skip)]
    pub desired: Arc<BTreeMap<String, InstanceSettings>>,
    #[serde(skip)]
    pub(crate) candidate_forwarders: Option<Arc<BTreeSet<String>>>,
    #[serde(skip)]
    pub active_references: BTreeMap<String, String>,
    #[serde(skip)]
    pub(crate) outcomes: Arc<Mutex<crate::control::BatchOutcomes>>,
}

#[derive(Clone)]
pub struct View {
    pub manager: Weak<Manager>,
    pub diagnostics: Arc<Mutex<Diagnostics>>,
    ids: Arc<Mutex<LibraryIds>>,
    libraries: Arc<Mutex<LibrarySample>>,
}

#[derive(Clone, Default)]
struct LibrarySample {
    libraries: Vec<LibraryInfo>,
    errors: BTreeMap<String, String>,
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
    pub last_batch: Vec<BatchOutcome>,
    pub core_version: &'static str,
    pub pid: u32,
    #[serde(flatten)]
    pub snapshot: Snapshot,
    #[serde(flatten)]
    pub diagnostics: Diagnostics,
    pub libraries: Vec<LibraryInfo>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub library_sample_errors: BTreeMap<String, String>,
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
            libraries: Arc::new(Mutex::new(LibrarySample::default())),
        }
    }

    /// ADR-0028：内核失败记录转换为接口诊断文本，其余字段保持真实来源。
    pub fn runtime_snapshot(&self) -> Result<RuntimeSnapshot> {
        let observed = self.snapshot()?;
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
            last_batch: observed.last_batch,
            library_sample_errors: observed.library_sample_errors,
        })
    }

    async fn sample_libraries(&self) -> Result<()> {
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| cordis_up::WireError::new(4, "核心已退出"))?;
        let (mut libraries, errors) = manager.sample_libraries().await;
        let mut sample = self.libraries.lock();
        libraries.extend(
            sample
                .libraries
                .drain(..)
                .filter(|library| errors.contains_key(&library.group)),
        );
        libraries.sort_by_key(|library| (library.connection, library.id));
        *sample = LibrarySample { libraries, errors };
        Ok(())
    }

    /// 读者组合当前内核视图和最近的宿主采样。
    pub fn snapshot(&self) -> Result<Status> {
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| cordis_up::WireError::new(4, "核心已退出"))?;
        let LibrarySample {
            mut libraries,
            errors: library_sample_errors,
        } = self.libraries.lock().clone();
        let (mut snapshot, last_swap) = self.map_library_ids(&manager, &mut libraries)?;
        let diagnostics = self.diagnostics.lock().clone();
        if let Some(web) = diagnostics.web.as_ref().and_then(Weak::upgrade) {
            let projections: BTreeMap<_, _> = diagnostics
                .desired
                .keys()
                .flat_map(|name| {
                    web.child_names(name)
                        .into_iter()
                        .map(|child| (child, name.clone()))
                })
                .collect();
            for instance in &mut snapshot.instances {
                instance.page_of = projections.get(&instance.name).cloned();
            }
            snapshot.instances.extend(web.snapshot());
        }
        complete_instances(&mut snapshot, &diagnostics);
        let last_batch = diagnostics.outcomes.lock().snapshot();
        Ok(Status {
            last_batch,
            core_version: env!("CARGO_PKG_VERSION"),
            pid: std::process::id(),
            snapshot,
            libraries,
            library_sample_errors,
            last_swap,
            available_plugins: diagnostics.sources.iter().map(package_info).collect(),
            diagnostics,
        })
    }

    /// ADR-0028：同一连接和本地库编号共享运行时编号，报告沿用相同映射。
    fn map_library_ids(
        &self,
        manager: &Manager,
        libraries: &mut Vec<LibraryInfo>,
    ) -> Result<(Snapshot, Vec<SwapReport>)> {
        let mut last_swap = Vec::new();
        let mut ids = self.ids.lock();
        let snapshot = manager.snapshot();
        let active: HashSet<_> = snapshot
            .groups
            .iter()
            .filter_map(|group| group.connection)
            .collect();
        let reports = manager.library_swap_reports();
        let reported: HashSet<_> = reports
            .iter()
            .flat_map(|(connection, report)| {
                [
                    (*connection, report.previous_library),
                    (*connection, report.candidate_library),
                ]
            })
            .collect();
        ids.known
            .retain(|key, _| active.contains(&key.0) || reported.contains(key));
        libraries.retain(|library| library.connection.is_none_or(|id| active.contains(&id)));
        for library in libraries {
            let connection = library
                .connection
                .ok_or_else(|| cordis_up::WireError::new(4, "宿主库报告缺少连接编号"))?;
            library.id = ids.get(connection, library.id);
        }
        for (connection, mut report) in reports {
            report.previous_library = ids.get(connection, report.previous_library);
            report.candidate_library = ids.get(connection, report.candidate_library);
            last_swap.push(report);
        }
        Ok((snapshot, last_swap))
    }
}

/// DOC-11 §10、ADR-0033：补充配置中保留的停用、等待和失败条目。
fn complete_instances(snapshot: &mut Snapshot, diagnostics: &Diagnostics) {
    let mut missing: BTreeMap<_, _> = diagnostics.desired.iter().collect();
    for instance in &mut snapshot.instances {
        if missing.remove(&instance.name).is_some() {
            instance.reference = diagnostics.active_references.get(&instance.name).cloned();
        }
    }
    for (name, settings) in missing {
        let failure = diagnostics.instance_errors.get(name);
        snapshot
            .instances
            .push(cordis_supervisor::manager::InstanceSnapshot {
                page_of: None,
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
        page_of: instance.page_of,
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
        discarded_confirmation: report.discarded_confirmation,
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

/// DOC-15 CP-11：快照变化时原子发布，写入错误交由发布任务记录。
pub async fn write(path: &Path, view: &View, previous: &mut Vec<u8>) -> Result<()> {
    view.sample_libraries().await?;
    let bytes = serde_json::to_vec_pretty(&view.snapshot()?).map_err(parameter)?;
    if *previous != bytes {
        storage::write_snapshot(path, &bytes)?;
        *previous = bytes;
    }
    Ok(())
}
