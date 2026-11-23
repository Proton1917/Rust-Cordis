//! ADR-0031、DOC-11 SW-1–SW-9：配置和候选版本共用批次预检、切换、确认及原状态恢复。

use super::*;
use futures_util::future::join_all;
use generation::{Binding, RemoteComponent};
use std::{borrow::Cow, collections::BTreeSet, path::PathBuf};

/// ADR-0031：原生候选按逻辑插件准备，进程候选携带完整启动计划。
pub enum BatchReplacement {
    Native {
        group: String,
        plugin: String,
        path: PathBuf,
    },
    Process(GroupSpec),
}

/// ADR-0031、ADR-0025：最终部署包含配置与派生视图，remount 标记上下文变化。
#[derive(Clone)]
pub struct BatchInstance {
    pub specification: InstanceSpec,
    pub version: String,
    pub view: Context,
    pub remount: bool,
}

/// ADR-0052 R-2：批次执行结果与核心形成的丢弃信息保持同一次操作的所有权。
pub struct BatchCommit {
    original: SharedStates,
    pub cleanup: Option<WireError>,
}

pub type BatchCompletion = std::result::Result<BatchCommit, BatchFailure>;

pub enum BatchFailure {
    Rejected(WireError),
    Restored {
        problem: WireError,
        discarded: Option<cordis_types::ConfirmationDiscard>,
    },
    RecoveryRequired(WireError),
}

impl From<WireError> for BatchFailure {
    fn from(problem: WireError) -> Self {
        Self::Rejected(problem)
    }
}

impl BatchFailure {
    fn problem(self) -> WireError {
        match self {
            Self::Rejected(problem)
            | Self::Restored { problem, .. }
            | Self::RecoveryRequired(problem) => problem,
        }
    }
}

pub enum DeploymentDecision {
    NotCommitted(WireError),
    Committed,
    RecoveryRequired(WireError),
}

type States = HashMap<String, Option<payload::State>>;
type SharedStates = HashMap<String, Option<Arc<payload::State>>>;
struct CommitWindow {
    candidate: oneshot::Sender<SharedStates>,
    decision: oneshot::Receiver<DeploymentDecision>,
}

/// 磁盘决定待恢复期间，旧连接和交接状态跟随原操作存活。
pub(super) struct HeldDeployment {
    groups: BTreeMap<String, PreparedGroup>,
    _transition: lifecycle::Transition,
}

#[derive(Clone, Copy)]
enum TargetSource<'a> {
    Manifest,
    Handshake,
    Restoration,
    Recovery(&'a [Weak<Instance>]),
}

struct LibraryChange {
    plugin: String,
    previous: u64,
    candidate: u64,
}

struct PreparedGroup {
    group: Arc<Group>,
    previous: Option<Arc<GroupGeneration>>,
    candidate: Arc<GroupGeneration>,
    specification: Option<LaunchPlan>,
    libraries: Vec<LibraryChange>,
}

impl Manager {
    /// 冷启动和契约往返恢复已经接受的声明，组件的激活由内核按依赖推进。
    pub async fn restore_instance(
        &self,
        target: BatchInstance,
        state: Option<payload::State>,
    ) -> Result<()> {
        self.transaction(move |manager| async move {
            let name = target.specification.name.clone();
            if manager.find_instance(&name).is_some() {
                return Err(error(2, "实例名称已经存在"));
            }
            let states = state.map(|state| (name, Some(state))).into_iter().collect();
            manager.restore_declared(vec![], vec![target], states).await
        })
        .await
    }

    pub(super) async fn restore_declared(
        &self,
        sources: Vec<BatchReplacement>,
        targets: Vec<BatchInstance>,
        states: States,
    ) -> Result<()> {
        self.execute_deployment_batch(
            sources,
            vec![],
            targets,
            states,
            true,
            TargetSource::Restoration,
            None,
        )
        .await
        .map_err(BatchFailure::problem)?
        .cleanup
        .map_or(Ok(()), Err)
    }

    fn batch_members(&self, group: &str) -> Result<Vec<BatchInstance>> {
        self.group_instances(group)
            .into_iter()
            .map(|instance| {
                let data = &instance.data;
                Ok(BatchInstance {
                    specification: InstanceSpec {
                        name: data.name.clone(),
                        group: data.group.clone(),
                        plugin: data.plugin.clone(),
                        component: data.component.clone(),
                        config: instance.configuration()?.config.as_ref().clone(),
                    },
                    version: String::new(),
                    view: instance.fiber.context(),
                    remount: false,
                })
            })
            .collect()
    }

    /// 失联分组以原启动计划进入同一部署事务，退避由连接监管器安排。
    pub(super) async fn retry_deployment(
        &self,
        specification: GroupSpec,
        cohort: &[Weak<Instance>],
    ) -> Result<()> {
        let members = self.batch_members(&specification.name)?;
        let committed = self
            .execute_deployment_batch(
                vec![BatchReplacement::Process(specification)],
                members.clone(),
                members,
                HashMap::new(),
                true,
                TargetSource::Recovery(cohort),
                None,
            )
            .await
            .map_err(BatchFailure::problem)?;
        if let Some(problem) = committed.cleanup {
            self.0
                .context
                .logger()
                .error(saved_error(&problem).to_string());
        }
        Ok(())
    }
    /// 按监管目录选取实际实例，包括由页面父效应持有的子实例。
    pub(super) async fn change_running_group(
        &self,
        group: &str,
        replacement: Option<BatchReplacement>,
        configuration: Option<(String, Value)>,
    ) -> Result<()> {
        self.group(group)?;
        let before = self.batch_members(group)?;
        let permit_pending = configuration.is_some();
        let mut target = before.clone();
        if let Some((name, config)) = configuration {
            let item = target
                .iter_mut()
                .find(|item| item.specification.name == name)
                .ok_or_else(|| error(4, "配置目标实例缺席"))?;
            item.specification.config = config;
        }
        let library = match &replacement {
            Some(BatchReplacement::Native { plugin, .. }) => Some(plugin.clone()),
            _ => None,
        };
        let result = self
            .execute_deployment_batch(
                replacement.into_iter().collect(),
                before,
                target,
                HashMap::new(),
                permit_pending,
                TargetSource::Handshake,
                None,
            )
            .await;
        match result {
            Ok(batch) => batch.cleanup.map_or(Ok(()), |problem| {
                Err(replacement_failure(
                    group,
                    library.as_deref(),
                    true,
                    problem,
                ))
            }),
            Err(BatchFailure::RecoveryRequired(problem)) => Err(problem),
            Err(failure) => {
                let mut problem = failure.problem();
                if library.is_some() {
                    problem =
                        Box::pin(self.after_failed_libraries(problem, [group.to_owned()])).await;
                }
                Err(replacement_failure(
                    group,
                    library.as_deref(),
                    false,
                    problem,
                ))
            }
        }
    }
    /// ADR-0031：配置回滚可能重建纤程，运行时从唯一目录刷新持有句柄。
    pub fn instance_fiber(&self, name: &str) -> Result<Fiber> {
        self.instance(name).map(|instance| instance.fiber.clone())
    }

    /// ADR-0031、A-7：队列持有整个窗口事务，等待者取消后继续提交或恢复。
    pub async fn replace_batch(
        &self,
        replacements: Vec<BatchReplacement>,
        before: Vec<BatchInstance>,
        target: Vec<BatchInstance>,
        states: HashMap<String, Option<payload::State>>,
    ) -> Result<HashMap<String, Option<payload::State>>> {
        match self
            .submit_batch(replacements, before, target, states, false, None)
            .await
        {
            Ok(BatchCommit {
                original,
                cleanup: None,
            }) => Ok(original
                .into_iter()
                .map(|(name, state)| (name, state.map(Arc::unwrap_or_clone)))
                .collect()),
            Ok(BatchCommit {
                cleanup: Some(problem),
                ..
            }) => Err(replacement_failure("batch", None, true, problem)),
            Err(BatchFailure::RecoveryRequired(problem)) => Err(problem),
            Err(failure) => Err(replacement_failure("batch", None, false, failure.problem())),
        }
    }

    pub async fn deploy<F, Fut>(
        &self,
        replacements: Vec<BatchReplacement>,
        before: Vec<BatchInstance>,
        target: Vec<BatchInstance>,
        permit_pending: bool,
        decide: F,
    ) -> BatchCompletion
    where
        F: FnOnce(SharedStates) -> Fut,
        Fut: std::future::Future<Output = DeploymentDecision>,
    {
        let (ready, candidate) = oneshot::channel();
        let (vote, decision) = oneshot::channel();
        let window = CommitWindow {
            candidate: ready,
            decision,
        };
        let worker = self.submit_batch(
            replacements,
            before,
            target,
            HashMap::new(),
            permit_pending,
            Some(window),
        );
        let client = async move {
            if let Ok(states) = candidate.await {
                let _ = vote.send(decide(states).await);
            }
        };
        tokio::join!(worker, client).0
    }

    async fn submit_batch(
        &self,
        replacements: Vec<BatchReplacement>,
        before: Vec<BatchInstance>,
        target: Vec<BatchInstance>,
        states: States,
        permit_pending: bool,
        window: Option<CommitWindow>,
    ) -> BatchCompletion {
        self.transaction(move |manager| async move {
            let groups: std::collections::BTreeSet<_> = replacements
                .iter()
                .filter_map(|item| match item {
                    BatchReplacement::Native { group, .. } => Some(group.clone()),
                    BatchReplacement::Process(_) => None,
                })
                .collect();
            let mut result = manager
                .execute_deployment_batch(
                    replacements,
                    before,
                    target,
                    states,
                    permit_pending,
                    TargetSource::Manifest,
                    window,
                )
                .await;
            if let Err(BatchFailure::Rejected(problem) | BatchFailure::Restored { problem, .. }) =
                &mut result
            {
                *problem = manager
                    .after_failed_libraries(problem.clone(), groups)
                    .await;
            }
            Ok(result)
        })
        .await
        .unwrap_or_else(|problem| Err(problem.into()))
    }

    /// SW-1–SW-5：候选和最终配置全部通过预检后才冻结旧实例。
    async fn execute_deployment_batch(
        &self,
        replacements: Vec<BatchReplacement>,
        before: Vec<BatchInstance>,
        mut target: Vec<BatchInstance>,
        states: HashMap<String, Option<payload::State>>,
        permit_pending: bool,
        source: TargetSource<'_>,
        window: Option<CommitWindow>,
    ) -> BatchCompletion {
        let started = std::time::Instant::now();
        if let Err(problem) = self.running() {
            return Err(if self.0.recovery_batch.lock().is_some() {
                BatchFailure::RecoveryRequired(problem)
            } else {
                BatchFailure::Rejected(problem)
            });
        }
        let (prepared, names) = self
            .prepare_batch_sources(replacements, &before, &mut target, permit_pending, source)
            .await?;
        let restoring = matches!(source, TargetSource::Restoration);
        let recovering = matches!(source, TargetSource::Recovery(_));
        let mut transition = if restoring {
            lifecycle::Transition::default()
        } else {
            match self.freeze_batch(&prepared, &target, &names, source).await {
                Ok(frozen) => frozen,
                Err(problem) => return Err(self.discard_batch(&prepared, problem).await.into()),
            }
        };
        transition.permit_pending = permit_pending;
        if !recovering {
            for group in prepared.values() {
                group.group.health.lock().state = "replacing".into();
            }
        }
        let applied = {
            let inherited = batch_inherited(&transition, &before, &target, states);
            self.apply_batch(&prepared, &before, &target, &inherited, false)
                .await
        };
        let accepted = match applied {
            Ok(()) if restoring => Ok((started.elapsed(), 0)),
            applied if recovering => {
                let settled = match applied {
                    Ok(()) => {
                        self.stabilize_with(&transition.before, &transition.names, true)
                            .await
                    }
                    Err(problem) => Err(problem),
                };
                let thawed = self.thaw_current(&transition.frozen).await;
                let problem = settled.err().or_else(|| thawed.err());
                for group in prepared.values() {
                    group.group.health.lock().error = problem.as_ref().map(saved_error);
                }
                Ok((started.elapsed(), transition.names.len()))
            }
            Ok(()) => {
                self.accept_batch(&prepared, &mut transition, &names, &target, started)
                    .await
            }
            Err(problem) => Err(problem),
        };
        let accepted = match (accepted, window) {
            (Ok(timing), Some(window)) => {
                let mut affected = names;
                affected.extend(transition.names.iter().cloned());
                let names: Vec<_> = target
                    .iter()
                    .filter(|item| {
                        item.view.fiber() == self.0.fiber
                            && affected.contains(&item.specification.name)
                    })
                    .map(|item| item.specification.name.clone())
                    .collect();
                let mut states = self.capture_deployment(&names).await;
                for old in &before {
                    let old = &old.specification;
                    if !target.iter().any(|item| {
                        let new = &item.specification;
                        new.name == old.name
                            && new.plugin == old.plugin
                            && new.component == old.component
                    }) {
                        states.entry(old.name.clone()).or_insert(None);
                    }
                }
                let _ = window.candidate.send(states);
                let mut decision = window.decision;
                let decision = loop {
                    tokio::select! {
                        biased;
                        outcome = &mut decision => break outcome.unwrap_or_else(|_| DeploymentDecision::NotCommitted(error(7, "部署决定方已退出"))),
                        Some(command) = self.next_page_command() => command.run(self.clone()).await,
                    }
                };
                match decision {
                    DeploymentDecision::Committed => Ok(timing),
                    DeploymentDecision::NotCommitted(problem) => Err(problem),
                    DeploymentDecision::RecoveryRequired(problem) => {
                        *self.0.recovery_batch.lock() = Some(HeldDeployment {
                            groups: prepared,
                            _transition: transition,
                        });
                        return Err(BatchFailure::RecoveryRequired(problem));
                    }
                }
            }
            (result, _) => result,
        };
        let (paused, affected) = match accepted {
            Ok(accepted) => accepted,
            Err(problem) => {
                return Err(self
                    .rollback_batch(
                        prepared,
                        &before,
                        &target,
                        transition,
                        started.elapsed(),
                        problem,
                    )
                    .await);
            }
        };
        if !recovering {
            self.record_batch_reports(&prepared, affected, paused, None, None, None);
        }
        let committed = self.commit_batch(&prepared, recovering).await;
        let original = std::mem::take(&mut transition.inherited);
        drop(transition);
        Ok(BatchCommit {
            original,
            cleanup: committed
                .and(
                    self.retire_batch(&prepared, &before, &target, recovering)
                        .await,
                )
                .err(),
        })
    }

    pub(super) async fn close_held_deployment(&self) -> Result<()> {
        let held = self.0.recovery_batch.lock().take();
        let mut result = Ok(());
        if let Some(held) = held {
            let active: HashSet<_> = self
                .0
                .catalog
                .lock()
                .groups
                .values()
                .map(|group| group.current.lock().connection.id())
                .collect();
            for group in held.groups.into_values() {
                let mut endpoints = HashSet::new();
                for version in group.previous.into_iter().chain([group.candidate]) {
                    let id = version.connection.id();
                    if !active.contains(&id) && endpoints.insert(id) {
                        result = result.and(self.retire(&version).await);
                    }
                }
            }
        }
        result
    }

    /// SW-1–SW-4：完整准备全部来源和重启计划，再按最终部署整体预检。
    async fn prepare_batch_sources(
        &self,
        replacements: Vec<BatchReplacement>,
        before: &[BatchInstance],
        target: &mut [BatchInstance],
        permit_pending: bool,
        source: TargetSource<'_>,
    ) -> Result<(BTreeMap<String, PreparedGroup>, HashSet<String>)> {
        let mut prepared = BTreeMap::new();
        let preparation = async {
            for replacement in replacements {
                self.prepare_batch_source(replacement, &mut prepared)
                    .await?;
            }
            for group in prepared
                .values_mut()
                .filter(|group| !group.libraries.is_empty())
            {
                group.specification = self
                    .restart_specification(&group.group, &group.candidate)
                    .await?;
            }
            self.preflight_batch(&prepared, before, target, permit_pending, source)
                .await
        }
        .await;
        match preparation {
            Ok(affected) => Ok((prepared, affected)),
            Err(problem) => Err(self.discard_batch(&prepared, problem).await),
        }
    }

    /// SW-5：联合候选和配置差异确定一次冻结集合，原快照属于整个窗口。
    async fn freeze_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        target: &[BatchInstance],
        names: &HashSet<String>,
        source: TargetSource<'_>,
    ) -> Result<lifecycle::Transition> {
        let mut incoming = target
            .iter()
            .filter(|item| names.contains(&item.specification.name))
            .map(|item| {
                let group = self.batch_generation(prepared, &item.specification.group, false)?;
                let component = group
                    .components
                    .get(&(
                        item.specification.plugin.clone(),
                        item.specification.component.clone(),
                    ))
                    .cloned()
                    .ok_or_else(|| error(3, "批次组件描述缺席"))?;
                Ok((
                    item.view.clone(),
                    component,
                    item.specification.name.clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut roots = names.clone();
        for group in prepared.values() {
            for instance in self.group_instances(&group.group.name) {
                let data = &instance.data;
                if (group.libraries.is_empty()
                    || group
                        .libraries
                        .iter()
                        .any(|library| library.plugin == data.plugin))
                    && roots.insert(data.name.clone())
                    && let Some(component) = group
                        .candidate
                        .components
                        .get(&(data.plugin.clone(), data.component.clone()))
                {
                    incoming.push((
                        instance.fiber.context(),
                        component.clone(),
                        data.name.clone(),
                    ));
                }
            }
        }
        if let TargetSource::Recovery(cohort) = source {
            roots.extend(
                cohort
                    .iter()
                    .filter_map(Weak::upgrade)
                    .filter_map(|instance| {
                        let name = &instance.data.name;
                        self.find_instance(name)
                            .filter(|current| Arc::ptr_eq(current, &instance))
                            .map(|_| name.clone())
                    }),
            );
        }
        self.prepare_transition(
            roots,
            incoming,
            !matches!(source, TargetSource::Recovery(_)),
        )
        .await
    }

    /// DOC-11 §7：恢复窗口前全部定义和状态后释放候选，并保留实际回滚报告。
    async fn rollback_batch(
        &self,
        prepared: BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        target: &[BatchInstance],
        transition: lifecycle::Transition,
        elapsed: Duration,
        problem: WireError,
    ) -> BatchFailure {
        let discarded = self.confirmation_discard(&transition);
        let restored = match self
            .apply_batch(&prepared, target, before, &transition.inherited, true)
            .await
        {
            Ok(()) => self
                .stabilize_with(
                    &transition.before,
                    &transition.names,
                    transition.permit_pending,
                )
                .await
                .map(|_| ()),
            Err(problem) => Err(problem),
        };
        let restored = match restored {
            Ok(()) => self.thaw_current(&transition.frozen).await,
            Err(problem) => Err(problem),
        };
        self.record_batch_reports(
            &prepared,
            transition.names.len(),
            elapsed,
            Some(restored.is_ok()),
            Some(&problem),
            discarded.as_ref(),
        );
        for group in prepared.values() {
            *group.group.health.lock() = Health {
                state: if restored.is_ok() {
                    "running"
                } else {
                    "failed"
                }
                .into(),
                error: Some(saved_error(&problem)),
            };
        }
        if let Err(recovery) = restored {
            *self.0.recovery_batch.lock() = Some(HeldDeployment {
                groups: prepared,
                _transition: transition,
            });
            return BatchFailure::RecoveryRequired(error(
                1,
                format!("批次失败：{problem}；原状态恢复失败：{recovery}"),
            ));
        }
        for group in prepared.values().filter(|group| group.previous.is_none()) {
            self.0.catalog.lock().groups.remove(&group.group.name);
        }
        BatchFailure::Restored {
            problem: self.discard_batch(&prepared, problem).await,
            discarded,
        }
    }

    /// SW-1–SW-3：同宿主的多个原生候选逐个合并描述符，保留其他组件定义身份。
    async fn prepare_batch_source(
        &self,
        replacement: BatchReplacement,
        prepared: &mut BTreeMap<String, PreparedGroup>,
    ) -> Result<()> {
        match replacement {
            BatchReplacement::Native {
                group,
                plugin,
                path,
            } => {
                self.prepare_batch_native(group, plugin, path, prepared)
                    .await
            }
            BatchReplacement::Process(specification) => {
                self.prepare_batch_process(specification, prepared).await
            }
        }
    }

    /// SW-1–SW-3：原生库准备后并入该组预备定义，前一库的准备结果继续持有。
    async fn prepare_batch_native(
        &self,
        group: String,
        plugin: String,
        path: PathBuf,
        prepared: &mut BTreeMap<String, PreparedGroup>,
    ) -> Result<()> {
        if !prepared.contains_key(&group) {
            let current = self.group(&group)?;
            let previous = current.current.lock().clone();
            prepared.insert(
                group.clone(),
                PreparedGroup {
                    group: current,
                    previous: Some(previous.clone()),
                    candidate: previous,
                    specification: None,
                    libraries: Vec::new(),
                },
            );
        }
        let selected = prepared.get_mut(&group).expect("prepared group retained");
        if selected.specification.is_some()
            || selected.libraries.iter().any(|item| item.plugin == plugin)
        {
            return Err(error(2, "批次目标重复或混用了宿主与原生库候选"));
        }
        let old = selected
            .candidate
            .descriptor
            .plugins
            .iter()
            .find(|item| item.name == plugin)
            .and_then(|item| item.library)
            .ok_or_else(|| error(3, "原生批次目标缺少活动库"))?;
        let library = self
            .prepare_library(&selected.candidate.connection, &path)
            .await?;
        selected.libraries.push(LibraryChange {
            plugin: plugin.clone(),
            previous: old,
            candidate: library.library,
        });
        selected.candidate =
            self.native_generation(&group, &plugin, &selected.candidate, &library)?;
        Ok(())
    }

    /// SW-1–SW-3、CP-7：进程候选独立握手，新分组在提交阶段进入活动目录。
    async fn prepare_batch_process(
        &self,
        specification: GroupSpec,
        prepared: &mut BTreeMap<String, PreparedGroup>,
    ) -> Result<()> {
        if prepared.contains_key(&specification.name) || specification.restart.max > 5 {
            return Err(error(2, "批次分组重复或重启次数超过5次"));
        }
        let (candidate, specification) = self.prepare_launch(specification).await?;
        let existing = self
            .0
            .catalog
            .lock()
            .groups
            .get(&specification.name)
            .cloned();
        let previous = existing.as_ref().map(|group| group.current.lock().clone());
        let group = existing.unwrap_or_else(|| {
            Arc::new(Group {
                name: specification.name.clone(),
                specification: Mutex::new(None),
                current: Mutex::new(candidate.clone()),
                definitions: Mutex::new(HashMap::new()),
                restarts: AtomicU32::new(0),
                health: Mutex::new(Health {
                    state: "replacing".into(),
                    error: None,
                }),
            })
        });
        prepared.insert(
            specification.name.clone(),
            PreparedGroup {
                group,
                previous,
                candidate,
                specification: Some(specification),
                libraries: Vec::new(),
            },
        );
        Ok(())
    }

    /// SW-4：最终实例配置与候选组件共同检查，接口可用性按最终视图和声明判断。
    async fn preflight_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        target: &mut [BatchInstance],
        permit_pending: bool,
        source: TargetSource<'_>,
    ) -> Result<HashSet<String>> {
        let mut components = HashMap::new();
        for item in target.iter_mut() {
            if matches!(source, TargetSource::Manifest | TargetSource::Restoration) {
                self.check_view(&item.view)?;
            }
            let specification = &mut item.specification;
            let generation = self.batch_generation(prepared, &specification.group, false)?;
            if specification.component.is_empty() {
                specification.component = select_component(&generation, &specification.plugin)?;
            }
            let component = generation
                .components
                .get(&(
                    specification.plugin.clone(),
                    specification.component.clone(),
                ))
                .ok_or_else(|| error(3, "批次目标缺少最终部署组件"))?
                .clone();
            if matches!(source, TargetSource::Manifest | TargetSource::Restoration)
                && generation
                    .descriptor
                    .plugins
                    .iter()
                    .find(|plugin| plugin.name == specification.plugin)
                    .is_none_or(|plugin| plugin.version != item.version)
            {
                return Err(error(3, "批次来源清单与握手版本冲突"));
            }
            if components
                .insert(specification.name.clone(), component)
                .is_some()
            {
                return Err(error(2, "批次最终实例名称重复"));
            }
        }
        let affected = batch_affected(prepared, before, target);
        if matches!(source, TargetSource::Handshake | TargetSource::Recovery(_)) {
            for group in prepared.values() {
                self.preflight_interfaces(&group.group, &group.candidate)?;
            }
        }
        for item in target
            .iter()
            .filter(|item| affected.contains(&item.specification.name))
        {
            self.resolve(
                &components[&item.specification.name],
                &item.specification.config,
            )
            .await?;
        }
        if permit_pending {
            return Ok(affected);
        }
        let mut provided = HashMap::new();
        for item in target.iter() {
            for interface in &components[&item.specification.name].provides {
                let key = (interface.key.clone(), item.view.realm_named(&interface.key));
                provided
                    .entry(key)
                    .and_modify(|minor: &mut u32| *minor = (*minor).max(interface.minor))
                    .or_insert(interface.minor);
            }
        }
        for item in target.iter() {
            if affected.contains(&item.specification.name)
                || self
                    .instance(&item.specification.name)
                    .is_ok_and(|instance| instance.fiber.state() == cordis::FiberState::Active)
            {
                self.preflight_batch_dependencies(
                    item,
                    &components[&item.specification.name],
                    &provided,
                    before,
                )?;
            }
        }
        Ok(affected)
    }

    /// ADR-0016、ADR-0025：合并候选提供声明后检查依赖，保留相同隔离域中的内置服务。
    fn preflight_batch_dependencies(
        &self,
        item: &BatchInstance,
        component: &RemoteComponent,
        provided: &HashMap<(String, cordis::Realm), u32>,
        before: &[BatchInstance],
    ) -> Result<()> {
        for (interface, required) in &component.inject {
            if !required {
                continue;
            }
            let key = (interface.key.clone(), item.view.realm_named(&interface.key));
            if provided
                .get(&key)
                .is_some_and(|minor| *minor >= interface.minor)
            {
                continue;
            }
            let existing = item.view.lookup_named(interface.key.clone());
            let external = existing
                .as_ref()
                .and_then(|value| value.downcast_ref::<Binding>())
                .is_some_and(|binding| {
                    binding.interface.minor >= interface.minor
                        && binding.generation.as_ref().is_none_or(|source| {
                            !before
                                .iter()
                                .any(|old| old.specification.name == source.instance.name)
                        })
                });
            if !external {
                return Err(error(
                    3,
                    format!(
                        "批次最终部署缺少 {} 所需的 {}",
                        item.specification.name, interface.full
                    ),
                ));
            }
        }
        Ok(())
    }

    /// SW-6：宿主候选全部激活后统一发布核心定义，再挂载最终实例和配置。
    async fn apply_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        target: &[BatchInstance],
        states: &SharedStates,
        rollback: bool,
    ) -> Result<()> {
        let mut recovered = Vec::new();
        for group in prepared.values() {
            if rollback && !group.candidate.connection.is_alive() {
                let transition = self
                    .prepare_transition(
                        self.group_instances(&group.group.name)
                            .into_iter()
                            .map(|i| i.data.name.clone()),
                        Vec::new(),
                        false,
                    )
                    .await?;
                self.inherit(&transition.inherited);
                recovered.push(transition);
                continue;
            }
            for library in &group.libraries {
                let id = if rollback {
                    library.previous
                } else {
                    library.candidate
                };
                self.library_command::<()>(&group.candidate.connection, "library_activate", id)
                    .await?;
            }
        }
        self.inherit(states);
        self.remove_batch_instances(before, target).await?;
        for group in prepared.values() {
            let version = if rollback {
                group.previous.clone()
            } else {
                Some(group.candidate.clone())
            };
            if let Some(version) = version {
                let version = if rollback {
                    self.rollback_version(&group.group, &version).await?
                } else {
                    version
                };
                self.0
                    .catalog
                    .lock()
                    .groups
                    .insert(group.group.name.clone(), group.group.clone());
                self.activate(&group.group, version)?;
            }
        }
        self.apply_batch_instances(prepared, target, states, rollback)
            .await?;
        for transition in recovered {
            self.stabilize_with(&transition.before, &transition.names, true)
                .await?;
            self.thaw_current(&transition.frozen).await?;
        }
        Ok(())
    }

    /// S-FBR-4、ADR-0031：所有版本已发布后调和实例配置，并等待全部配置更新任务。
    async fn apply_batch_instances(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        target: &[BatchInstance],
        states: &SharedStates,
        rollback: bool,
    ) -> Result<()> {
        let mut updated = Vec::new();
        for item in target {
            let specification = &item.specification;
            let existing = self.find_instance(&specification.name);
            if let Some(instance) = existing {
                if instance.configuration()?.config.as_ref() != &specification.config {
                    updated
                        .push(instance.set_configuration(Arc::new(specification.config.clone())));
                }
            } else if item.view.fiber().is_alive() {
                let generation = if rollback {
                    self.group(&specification.group)?.current.lock().clone()
                } else {
                    self.batch_generation(prepared, &specification.group, false)?
                };
                let definition = generation
                    .definitions
                    .get(&(
                        specification.plugin.clone(),
                        specification.component.clone(),
                    ))
                    .ok_or_else(|| error(3, "批次组件定义缺席"))?
                    .clone();
                self.install_instance(
                    item.view.clone(),
                    specification.clone(),
                    definition,
                    states.get(&specification.name).cloned().flatten(),
                )?;
            }
        }
        for outcome in join_all(updated).await {
            outcome?;
        }
        Ok(())
    }

    /// T-EFF-2：先释放移除及上下文变化的实例，再允许相同名称重新挂载。
    async fn remove_batch_instances(
        &self,
        before: &[BatchInstance],
        target: &[BatchInstance],
    ) -> Result<()> {
        let removed = before
            .iter()
            .filter(|old| {
                target
                    .iter()
                    .find(|next| next.specification.name == old.specification.name)
                    .is_none_or(|next| remount(old, next))
            })
            .collect::<Vec<_>>();
        let mut tasks = Vec::new();
        for item in &removed {
            let instance = self.find_instance(&item.specification.name);
            if let Some(instance) = instance {
                tasks.push(instance.fiber.dispose());
            }
        }
        let results = join_all(tasks).await;
        {
            let mut catalog = self.0.catalog.lock();
            for item in removed {
                catalog.instances.remove(&item.specification.name);
            }
        }
        for result in results {
            result.map_err(kernel_error)?;
        }
        Ok(())
    }

    /// SW-7/SW-8：所有候选已生效后统一等待依赖稳定，并同时观察各连接的确认期。
    async fn accept_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        transition: &mut lifecycle::Transition,
        names: &HashSet<String>,
        target: &[BatchInstance],
        started: std::time::Instant,
    ) -> Result<(Duration, usize)> {
        let mut selected: BTreeSet<_> = target
            .iter()
            .filter(|item| names.contains(&item.specification.name))
            .map(|item| item.specification.name.clone())
            .collect();
        selected.extend(transition.names.iter().cloned());
        let selected = selected.into_iter().collect::<Vec<_>>();
        let changed = self
            .stabilize_with(&transition.before, &selected, transition.permit_pending)
            .await?;
        let paused = started.elapsed();
        let connections = prepared
            .values()
            .map(|group| group.candidate.connection.clone())
            .collect::<Vec<_>>();
        self.confirm_transition(transition, &changed, &connections)
            .await?;
        Ok((paused, changed.len()))
    }

    /// SW-9：保存全批提交事实，进程监管继续指向本次活动版本。
    async fn commit_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        recovering: bool,
    ) -> Result<()> {
        for group in prepared.values() {
            self.replace_plan(&group.group, group.specification.clone())
                .await;
            {
                let mut health = group.group.health.lock();
                health.state = "running".into();
                if !recovering {
                    health.error = None;
                    group.group.restarts.store(0, Ordering::Release);
                }
            }
            if group.libraries.is_empty() {
                self.monitor(group.group.clone(), group.candidate.clone())?;
            }
        }
        Ok(())
    }

    /// SW-9、CP-7：提交后释放旧库和退出的分组，全部独立清理都得到执行。
    async fn retire_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        target: &[BatchInstance],
        recovering: bool,
    ) -> Result<()> {
        let mut failure = None;
        for group in prepared.values() {
            let retired = if group.libraries.is_empty() {
                match &group.previous {
                    Some(previous) => self.retire(previous).await,
                    None => Ok(()),
                }
            } else {
                self.retire_batch_libraries(group).await
            };
            if !recovering
                && let Some(report) = self.0.catalog.lock().reports.get_mut(&group.group.name)
            {
                report.succeeded = retired.is_ok();
                report.failure = retired.as_ref().err().map(Failure::from);
            }
            if let Err(problem) = retired {
                failure.get_or_insert(problem);
            }
        }
        let removed = before
            .iter()
            .map(|item| item.specification.group.clone())
            .filter(|group| !target.iter().any(|item| item.specification.group == *group))
            .collect::<HashSet<_>>();
        for name in removed {
            if !self.group_instances(&name).is_empty() {
                continue;
            }
            let group = self.0.catalog.lock().groups.remove(&name);
            if let Some(group) = group {
                let previous = group.current.lock().clone();
                if let Err(problem) = self.retire(&previous).await {
                    failure.get_or_insert(problem);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// SW-9：每个原生候选保存真实退役结果，完成该组清理后执行映像阈值策略。
    async fn retire_batch_libraries(&self, group: &PreparedGroup) -> Result<()> {
        let mut failure = None;
        let mut retired_images = 0;
        let mut retirement = None;
        for library in &group.libraries {
            let result = self
                .library_command::<payload::LibraryRetired>(
                    &group.candidate.connection,
                    "library_retire",
                    library.previous,
                )
                .await
                .and_then(|retired| {
                    retired_images = retired_images.max(retired.retired_images);
                    retirement = retired.retirement;
                    if retired.leases == 0 {
                        Ok(retired.quiescence_ms)
                    } else {
                        Err(error(6, "批次旧库仍有活动租约"))
                    }
                });
            if let Some((_, report)) = self
                .0
                .catalog
                .lock()
                .native_reports
                .get_mut(&(group.group.name.clone(), library.plugin.clone()))
            {
                report.succeeded = result.is_ok();
                report.quiescence_ms = result.as_ref().ok().copied().flatten();
                report.failure = result.as_ref().err().map(|e| metadata::clipped(&e.message));
            }
            if let Err(problem) = result {
                failure.get_or_insert(problem);
            }
        }
        if let Some(problem) = failure {
            return Err(self
                .after_failed_libraries(problem, [group.group.name.clone()])
                .await);
        }
        let restarted = self
            .enforce_image_limit(
                group.specification.clone(),
                retired_images,
                retirement.as_ref(),
            )
            .await;
        self.record_image_restart(group, &restarted);
        restarted.map(|_| ())
    }

    /// RM-3/RM-4：阈值重启属于本次换库，换库报告计入重启的暂停与受影响实例。
    fn record_image_restart(
        &self,
        group: &PreparedGroup,
        restarted: &Result<Option<ReplaceReport>>,
    ) {
        let mut catalog = self.0.catalog.lock();
        for library in &group.libraries {
            let Some((_, report)) = catalog
                .native_reports
                .get_mut(&(group.group.name.clone(), library.plugin.clone()))
            else {
                continue;
            };
            match restarted {
                Ok(Some(restart)) => {
                    report.pause_ms = report.pause_ms.saturating_add(restart.pause_ms);
                    report.affected_instances =
                        report.affected_instances.max(restart.affected_instances);
                }
                Ok(None) => {}
                Err(problem) => {
                    report.succeeded = false;
                    report.failure = Some(metadata::clipped(&problem.message));
                }
            }
        }
    }

    /// DOC-11 SW-9：报告记录本批实际影响实例数、提交事实与恢复结果。
    fn record_batch_reports(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        affected: usize,
        elapsed: Duration,
        rollback: Option<bool>,
        failure: Option<&WireError>,
        discarded: Option<&cordis_types::ConfirmationDiscard>,
    ) {
        let committed = rollback.is_none();
        let rolled_back = rollback.unwrap_or(false);
        let mut catalog = self.0.catalog.lock();
        for group in prepared.values() {
            let Some(previous) = &group.previous else {
                continue;
            };
            catalog.reports.insert(
                group.group.name.clone(),
                ReplaceReport {
                    group: group.group.name.clone(),
                    previous_connection: previous.connection.id(),
                    candidate_connection: group.candidate.connection.id(),
                    affected_instances: affected,
                    pause_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
                    committed,
                    succeeded: false,
                    rolled_back,
                    failure: failure.map(Failure::from),
                    discarded_confirmation: discarded.cloned(),
                },
            );
            for library in &group.libraries {
                catalog.native_reports.insert(
                    (group.group.name.clone(), library.plugin.clone()),
                    (
                        group.candidate.connection.id(),
                        cordis_types::SwapReport {
                            plugin: library.plugin.clone(),
                            previous_library: library.previous,
                            candidate_library: library.candidate,
                            affected_instances: affected,
                            pause_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
                            quiescence_ms: None,
                            committed,
                            succeeded: false,
                            rolled_back,
                            failure: failure.map(|failure| metadata::clipped(&failure.message)),
                            discarded_confirmation: discarded.cloned(),
                        },
                    ),
                );
            }
        }
    }

    /// SW-1–SW-4、CP-7：候选退出与库退役覆盖全部已准备来源，保留首个清理错误。
    async fn discard_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        mut problem: WireError,
    ) -> WireError {
        for group in prepared.values() {
            let cleanup = if group.libraries.is_empty() {
                if group
                    .previous
                    .as_ref()
                    .is_some_and(|old| Arc::ptr_eq(old, &group.candidate))
                {
                    continue;
                }
                self.retire(&group.candidate).await
            } else {
                if !group.candidate.connection.is_alive() {
                    continue;
                }
                let mut failure = None;
                for library in &group.libraries {
                    if let Err(error) = self
                        .library_command::<payload::LibraryRetired>(
                            &group.candidate.connection,
                            "library_retire",
                            library.candidate,
                        )
                        .await
                    {
                        failure.get_or_insert(error);
                    }
                }
                failure.map_or(Ok(()), Err)
            };
            if let Err(cleanup) = cleanup {
                problem.message = format!("{}；候选清理失败：{cleanup}", problem.message);
            }
        }
        problem
    }

    /// SW-4/SW-6：按事务方向读取预备定义，批次外分组继续使用当前定义。
    fn batch_generation(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        group: &str,
        rollback: bool,
    ) -> Result<Arc<GroupGeneration>> {
        if let Some(group) = prepared.get(group) {
            return if rollback {
                group
                    .previous
                    .clone()
                    .ok_or_else(|| error(4, "新增分组缺少旧版本"))
            } else {
                Ok(group.candidate.clone())
            };
        }
        Ok(self.group(group)?.current.lock().clone())
    }
}

/// DOC-15 §8：沿部署规则选择单一组件或与插件同名的组件。
fn select_component(group: &GroupGeneration, plugin: &str) -> Result<String> {
    let plugin = group
        .descriptor
        .plugins
        .iter()
        .find(|item| item.name == plugin)
        .ok_or_else(|| error(3, "批次来源缺少逻辑插件"))?;
    if plugin.components.len() == 1 {
        return Ok(plugin.components[0].name.clone());
    }
    plugin
        .components
        .iter()
        .find(|item| item.name == plugin.name)
        .map(|item| item.name.clone())
        .ok_or_else(|| error(2, "多组件插件需要指定 component"))
}

/// loader Entry.update、ADR-0025：上下文或组件身份变化触发重挂载。
fn remount(old: &BatchInstance, next: &BatchInstance) -> bool {
    old.remount
        || next.remount
        || old.specification.group != next.specification.group
        || old.specification.plugin != next.specification.plugin
        || old.specification.component != next.specification.component
}

/// SW-5：导出范围覆盖候选插件及配置变化实例，同组其他原生组件保持原纪元。
fn batch_affected(
    prepared: &BTreeMap<String, PreparedGroup>,
    before: &[BatchInstance],
    target: &[BatchInstance],
) -> HashSet<String> {
    let mut previous: HashMap<_, _> = before
        .iter()
        .map(|item| (&item.specification.name, item))
        .collect();
    let mut affected = HashSet::new();
    for item in target {
        let spec = &item.specification;
        let old = previous.remove(&spec.name);
        let candidate = prepared.get(&spec.group).is_some_and(|group| {
            group.libraries.is_empty()
                || group
                    .libraries
                    .iter()
                    .any(|library| library.plugin == spec.plugin)
        });
        if candidate
            || old.is_none_or(|old| remount(old, item) || old.specification.config != spec.config)
        {
            affected.insert(spec.name.clone());
        }
    }
    affected.extend(previous.into_keys().cloned());
    affected
}

/// DOC-11 §5：同逻辑组件沿原状态继承，配置更换组件时使用目标组件初值。
fn batch_inherited<'a>(
    transition: &'a lifecycle::Transition,
    before: &[BatchInstance],
    target: &[BatchInstance],
    overrides: HashMap<String, Option<payload::State>>,
) -> Cow<'a, SharedStates> {
    let mut inherited = Cow::Borrowed(&transition.inherited);
    for item in target {
        if before
            .iter()
            .find(|old| old.specification.name == item.specification.name)
            .is_some_and(|old| {
                old.specification.plugin != item.specification.plugin
                    || old.specification.component != item.specification.component
            })
        {
            inherited
                .to_mut()
                .insert(item.specification.name.clone(), None);
        }
    }
    if !overrides.is_empty() {
        inherited.to_mut().extend(
            overrides
                .into_iter()
                .map(|(name, state)| (name, state.map(Arc::new))),
        );
    }
    inherited
}
