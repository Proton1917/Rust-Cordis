//! ADR-0031、DOC-11 SW-1–SW-9：配置和候选版本共用批次预检、切换、确认及原状态恢复。

use super::*;
use futures_util::future::join_all;
use generation::{Binding, Configuration, RemoteComponent};
use std::path::PathBuf;

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

struct LibraryChange {
    plugin: String,
    previous: u64,
    candidate: u64,
}

struct PreparedGroup {
    group: Arc<Group>,
    previous: Option<Arc<GroupGeneration>>,
    candidate: Arc<GroupGeneration>,
    specification: Option<GroupSpec>,
    libraries: Vec<LibraryChange>,
}

impl Manager {
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
        let committed = Arc::new(AtomicBool::new(false));
        let marker = committed.clone();
        self.transaction(move |manager| async move {
            manager
                .execute_deployment_batch(replacements, before, target, states, marker)
                .await
        })
        .await
        .map_err(|problem| {
            replacement_failure("batch", None, committed.load(Ordering::Acquire), problem)
        })
    }

    /// SW-1–SW-5：候选和最终配置全部通过预检后才冻结旧实例。
    async fn execute_deployment_batch(
        &self,
        replacements: Vec<BatchReplacement>,
        before: Vec<BatchInstance>,
        mut target: Vec<BatchInstance>,
        states: HashMap<String, Option<payload::State>>,
        committed: Arc<AtomicBool>,
    ) -> Result<HashMap<String, Option<payload::State>>> {
        self.running()?;
        let prepared = self
            .prepare_batch_sources(replacements, &before, &mut target)
            .await?;
        let (transition, names) = match self.freeze_batch(&prepared, &before, &target).await {
            Ok(frozen) => frozen,
            Err(problem) => return Err(self.discard_batch(&prepared, problem).await),
        };
        let original = transition
            .names
            .iter()
            .map(|name| (name.clone(), transition.frozen.states.get(name).cloned()))
            .collect::<HashMap<_, _>>();
        let mut inherited = batch_inherited(&transition, &before, &target);
        inherited.extend(states);
        let started = std::time::Instant::now();
        let accepted = match self
            .apply_batch(&prepared, &before, &target, &inherited, false)
            .await
        {
            Ok(()) => {
                self.accept_batch(&prepared, &transition.before, &names, &target)
                    .await
            }
            Err(problem) => Err(problem),
        };
        if let Err(problem) = accepted {
            return Err(self
                .rollback_batch(
                    &prepared,
                    &before,
                    &target,
                    transition,
                    started.elapsed(),
                    problem,
                )
                .await);
        }
        committed.store(true, Ordering::Release);
        self.record_batch_reports(&prepared, names.len(), started.elapsed(), true, false, None);
        self.commit_batch(&prepared, &before, &target, &transition)
            .await?;
        drop(transition);
        self.retire_batch(&prepared, &before, &target).await?;
        Ok(original)
    }

    /// SW-1–SW-4：完整准备全部来源和重启计划，再按最终部署整体预检。
    async fn prepare_batch_sources(
        &self,
        replacements: Vec<BatchReplacement>,
        before: &[BatchInstance],
        target: &mut [BatchInstance],
    ) -> Result<BTreeMap<String, PreparedGroup>> {
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
            self.preflight_batch(&prepared, before, target).await
        }
        .await;
        if let Err(problem) = preparation {
            return Err(self.discard_batch(&prepared, problem).await);
        }
        Ok(prepared)
    }

    /// SW-5：联合候选和配置差异确定一次冻结集合，原快照属于整个窗口。
    async fn freeze_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        target: &[BatchInstance],
    ) -> Result<(lifecycle::Transition, HashSet<String>)> {
        let names = batch_affected(prepared, before, target);
        let instances = before
            .iter()
            .filter(|item| names.contains(&item.specification.name))
            .map(|item| self.instance(&item.specification.name))
            .collect::<Result<Vec<_>>>()?;
        Ok((self.prepare_transition(&instances).await?, names))
    }

    /// DOC-11 §7：恢复窗口前全部定义和状态后释放候选，并保留实际回滚报告。
    async fn rollback_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        target: &[BatchInstance],
        transition: lifecycle::Transition,
        elapsed: Duration,
        problem: WireError,
    ) -> WireError {
        let original = transition
            .names
            .iter()
            .map(|name| (name.clone(), transition.frozen.states.get(name).cloned()))
            .collect::<HashMap<_, _>>();
        let restored = match self
            .apply_batch(prepared, target, before, &original, true)
            .await
        {
            Ok(()) => self
                .stabilize(&transition.before, &transition.names)
                .await
                .map(|_| ()),
            Err(problem) => Err(problem),
        };
        self.record_batch_reports(
            prepared,
            transition.names.len(),
            elapsed,
            false,
            restored.is_ok(),
            Some(&problem),
        );
        drop(transition);
        self.batch_failed(prepared, problem, restored).await
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
        let candidate = self.prepare_process(&specification).await?;
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
    ) -> Result<()> {
        let mut components = HashMap::new();
        for item in target.iter_mut() {
            self.check_view(&item.view)?;
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
            if generation
                .descriptor
                .plugins
                .iter()
                .find(|plugin| plugin.name == specification.plugin)
                .is_none_or(|plugin| plugin.version != item.version)
            {
                return Err(error(3, "批次来源清单与握手版本冲突"));
            }
            self.resolve(&component, &specification.config).await?;
            if components
                .insert(specification.name.clone(), component)
                .is_some()
            {
                return Err(error(2, "批次最终实例名称重复"));
            }
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
        let affected = batch_affected(prepared, before, target);
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
        Ok(())
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
        states: &HashMap<String, Option<payload::State>>,
        rollback: bool,
    ) -> Result<()> {
        for group in prepared.values() {
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
        self.remove_batch_instances(before, target).await?;
        self.inherit(states);
        for group in prepared.values() {
            let version = if rollback {
                group.previous.clone()
            } else {
                Some(group.candidate.clone())
            };
            if let Some(version) = version {
                self.0
                    .catalog
                    .lock()
                    .groups
                    .insert(group.group.name.clone(), group.group.clone());
                group.group.health.lock().state = "replacing".into();
                self.activate(&group.group, version, None)?;
            }
        }
        self.apply_batch_instances(prepared, target, states, rollback)
            .await
    }

    /// S-FBR-4、ADR-0031：所有版本已发布后调和实例配置，并等待全部配置更新任务。
    async fn apply_batch_instances(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        target: &[BatchInstance],
        states: &HashMap<String, Option<payload::State>>,
        rollback: bool,
    ) -> Result<()> {
        let mut updated = Vec::new();
        for item in target {
            let specification = &item.specification;
            let existing = self
                .0
                .catalog
                .lock()
                .instances
                .get(&specification.name)
                .cloned();
            if let Some(instance) = existing {
                if *instance.data.config.lock() != specification.config {
                    *instance.data.config.lock() = specification.config.clone();
                    updated.push(instance.fiber.update(Configuration {
                        instance: instance.data.clone(),
                        config: specification.config.clone(),
                    }));
                }
            } else {
                let generation = self.batch_generation(prepared, &specification.group, rollback)?;
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
            outcome.map_err(kernel_error)?;
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
            let instance = self
                .0
                .catalog
                .lock()
                .instances
                .get(&item.specification.name)
                .cloned();
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
        epochs: &HashMap<String, cordis::Epoch>,
        names: &HashSet<String>,
        target: &[BatchInstance],
    ) -> Result<()> {
        let names = target
            .iter()
            .filter(|item| names.contains(&item.specification.name))
            .map(|item| item.specification.name.clone())
            .collect::<Vec<_>>();
        let changed = self.stabilize(epochs, &names).await?;
        for result in join_all(
            prepared
                .values()
                .map(|group| self.confirm(&changed, &group.candidate.connection)),
        )
        .await
        {
            result?;
        }
        Ok(())
    }

    /// SW-9：保存全批提交事实和原快照，进程监管继续指向本次活动版本。
    async fn commit_batch(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        before: &[BatchInstance],
        _target: &[BatchInstance],
        transition: &lifecycle::Transition,
    ) -> Result<()> {
        let previous_groups = before
            .iter()
            .map(|item| item.specification.group.clone())
            .collect::<HashSet<_>>();
        for name in previous_groups {
            let names = before
                .iter()
                .filter(|item| {
                    item.specification.group == name
                        && transition.names.contains(&item.specification.name)
                })
                .map(|item| item.specification.name.clone())
                .collect::<Vec<_>>();
            self.retain_replacement_states(&name, &names, &transition.frozen);
        }
        for group in prepared.values() {
            *group.group.specification.lock() = group.specification.clone();
            *group.group.health.lock() = Health {
                state: "running".into(),
                error: None,
            };
            group.group.restarts.store(0, Ordering::Release);
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
            if let Some(report) = self.0.catalog.lock().reports.get_mut(&group.group.name) {
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
        for library in &group.libraries {
            match self
                .library_command::<payload::LibraryRetired>(
                    &group.candidate.connection,
                    "library_retire",
                    library.previous,
                )
                .await
            {
                Ok(retired) if retired.leases == 0 => {
                    retired_images = retired_images.max(retired.retired_images);
                    if let Some((_, report)) = self
                        .0
                        .catalog
                        .lock()
                        .native_reports
                        .get_mut(&(group.group.name.clone(), library.plugin.clone()))
                    {
                        report.succeeded = true;
                        report.quiescence_ms = retired.quiescence_ms;
                    }
                }
                Ok(_) => {
                    failure.get_or_insert_with(|| error(6, "批次旧库仍有活动租约"));
                }
                Err(problem) => {
                    failure.get_or_insert(problem);
                }
            }
        }
        if let Err(problem) = self
            .enforce_image_limit(
                &group.group.name,
                group.specification.clone(),
                retired_images,
            )
            .await
        {
            failure.get_or_insert(problem);
        }
        failure.map_or(Ok(()), Err)
    }

    /// DOC-11 SW-9：报告记录本批实际影响实例数、提交事实与恢复结果。
    fn record_batch_reports(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        affected: usize,
        elapsed: Duration,
        committed: bool,
        rolled_back: bool,
        failure: Option<&WireError>,
    ) {
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
                    committed,
                    succeeded: false,
                    rolled_back,
                    failure: failure.map(Failure::from),
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
                        },
                    ),
                );
            }
        }
    }

    /// DOC-11 §7：回滚结果和候选清理错误共同保存，已恢复分组重新进入运行状态。
    async fn batch_failed(
        &self,
        prepared: &BTreeMap<String, PreparedGroup>,
        problem: WireError,
        restored: Result<()>,
    ) -> WireError {
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
            if group.previous.is_none() {
                self.0.catalog.lock().groups.remove(&group.group.name);
            }
        }
        let problem = match restored {
            Ok(()) => problem,
            Err(restored) => error(
                1,
                format!("批次失败：{problem}；原状态恢复失败：{restored}"),
            ),
        };
        self.discard_batch(prepared, problem).await
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
    before
        .iter()
        .chain(target)
        .filter(|item| {
            let spec = &item.specification;
            let candidate = prepared.get(&spec.group).is_some_and(|group| {
                group.libraries.is_empty()
                    || group
                        .libraries
                        .iter()
                        .any(|library| library.plugin == spec.plugin)
            });
            let old = before
                .iter()
                .find(|entry| entry.specification.name == spec.name);
            let next = target
                .iter()
                .find(|entry| entry.specification.name == spec.name);
            candidate
                || old.zip(next).is_none_or(|(old, next)| {
                    remount(old, next) || old.specification.config != next.specification.config
                })
        })
        .map(|item| item.specification.name.clone())
        .collect()
}

/// DOC-11 §5：同逻辑组件沿原状态继承，配置更换组件时使用目标组件初值。
fn batch_inherited(
    transition: &lifecycle::Transition,
    before: &[BatchInstance],
    target: &[BatchInstance],
) -> HashMap<String, Option<payload::State>> {
    let mut inherited = transition.inherited.clone();
    for item in target {
        if before
            .iter()
            .find(|old| old.specification.name == item.specification.name)
            .is_some_and(|old| {
                old.specification.plugin != item.specification.plugin
                    || old.specification.component != item.specification.component
            })
        {
            inherited.insert(item.specification.name.clone(), None);
        }
    }
    inherited
}
