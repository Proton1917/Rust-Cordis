//! ADR-0031、DOC-11 SW-1–SW-9：配置和候选版本共用批次预检、切换、确认及原状态恢复。

use super::*;
use futures_util::future::join_all;
use generation::{Binding, RemoteComponent};
use std::{borrow::Cow, collections::BTreeSet, path::PathBuf};

mod apply;
mod prepare;

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

struct DeploymentBatch<'a> {
    replacements: Vec<BatchReplacement>,
    before: Vec<BatchInstance>,
    target: Vec<BatchInstance>,
    states: HashMap<String, Option<payload::State>>,
    permit_pending: bool,
    source: TargetSource<'a>,
    window: Option<CommitWindow>,
}

/// Immutable indexes over a prepared batch; references keep the original order
/// and views in the source vectors while identity matching uses direct lookup.
struct BatchIndex<'a> {
    by_name: HashMap<&'a str, &'a BatchInstance>,
    groups: HashSet<&'a str>,
}

impl<'a> BatchIndex<'a> {
    fn new(instances: &'a [BatchInstance]) -> Self {
        let mut by_name = HashMap::with_capacity(instances.len());
        let mut groups = HashSet::new();
        for instance in instances {
            by_name
                .entry(instance.specification.name.as_str())
                .or_insert(instance);
            groups.insert(instance.specification.group.as_str());
        }
        Self { by_name, groups }
    }

    fn get(&self, name: &str) -> Option<&'a BatchInstance> {
        #[cfg(test)]
        MATCH_LOOKUPS.with(|count| count.set(count.get() + 1));
        self.by_name.get(name).copied()
    }

    fn same_component(&self, old: &InstanceSpec) -> bool {
        self.get(&old.name).is_some_and(|next| {
            next.specification.plugin == old.plugin && next.specification.component == old.component
        })
    }
}

#[cfg(test)]
thread_local! { static MATCH_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

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
        self.execute_deployment_batch(DeploymentBatch {
            replacements: sources,
            before: vec![],
            target: targets,
            states,
            permit_pending: true,
            source: TargetSource::Restoration,
            window: None,
        })
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
            .execute_deployment_batch(DeploymentBatch {
                replacements: vec![BatchReplacement::Process(specification)],
                before: members.clone(),
                target: members,
                states: HashMap::new(),
                permit_pending: true,
                source: TargetSource::Recovery(cohort),
                window: None,
            })
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
            .execute_deployment_batch(DeploymentBatch {
                replacements: replacement.into_iter().collect(),
                before,
                target,
                states: HashMap::new(),
                permit_pending,
                source: TargetSource::Handshake,
                window: None,
            })
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
                .execute_deployment_batch(DeploymentBatch {
                    replacements,
                    before,
                    target,
                    states,
                    permit_pending,
                    source: TargetSource::Manifest,
                    window,
                })
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
    async fn execute_deployment_batch(&self, batch: DeploymentBatch<'_>) -> BatchCompletion {
        let DeploymentBatch {
            replacements,
            before,
            mut target,
            states,
            permit_pending,
            source,
            window,
        } = batch;
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
        let before_index = BatchIndex::new(&before);
        let target_index = BatchIndex::new(&target);
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
            let inherited = prepare::batch_inherited(&transition, &before_index, &target, states);
            self.apply_batch(
                &prepared,
                &before,
                &target,
                &target_index,
                &inherited,
                false,
            )
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
                    if !target_index.same_component(old) {
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
                    self.retire_batch(&prepared, &before, &target_index, recovering)
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
}

#[cfg(test)]
mod index_tests {
    use super::*;
    use cordis_testkit::TestExecutor;

    #[test]
    fn component_matching_and_inheritance_visit_each_target_once() {
        let context = Context::new(Arc::new(TestExecutor::default()));
        for count in [100, 1_000, 10_000] {
            let before: Vec<_> = (0..count)
                .map(|index| BatchInstance {
                    specification: InstanceSpec {
                        name: format!("instance-{index}"),
                        group: format!("group-{}", index % 4),
                        plugin: "plugin".into(),
                        component: "component".into(),
                        config: Value::Null,
                    },
                    version: "1.0.0".into(),
                    view: context.clone(),
                    remount: false,
                })
                .collect();
            let mut target = before.clone();
            target.reverse();
            target[0].specification.component = "replacement".into();
            target.pop();
            let old = BatchIndex::new(&before);
            let next = BatchIndex::new(&target);
            MATCH_LOOKUPS.with(|lookups| lookups.set(0));
            let changed: Vec<_> = before
                .iter()
                .filter(|item| !next.same_component(&item.specification))
                .collect();
            assert_eq!(changed.len(), 2);
            let transition = lifecycle::Transition::default();
            let inherited = prepare::batch_inherited(&transition, &old, &target, HashMap::new());
            assert_eq!(inherited.len(), 1);
            assert!(inherited.contains_key(&target[0].specification.name));
            let lookups = MATCH_LOOKUPS.with(|lookups| lookups.get());
            assert_eq!(lookups, before.len() + target.len());
            eprintln!("deployment-index instances={count} identity_lookups={lookups}");
        }
    }
}
