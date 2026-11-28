//! DOC-11 SW-1–SW-5、ADR-0031：批次来源准备、最终部署预检与影响冻结规划。
use super::*;

impl Manager {
    /// SW-1–SW-4：完整准备全部来源和重启计划，再按最终部署整体预检。
    pub(super) async fn prepare_batch_sources(
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
    pub(super) async fn freeze_batch(
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
        let before_names: HashSet<_> = before
            .iter()
            .map(|item| item.specification.name.as_str())
            .collect();
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
                    &before_names,
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
        before: &HashSet<&str>,
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
                        && binding
                            .generation
                            .as_ref()
                            .is_none_or(|source| !before.contains(source.instance.name.as_str()))
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

    /// SW-4/SW-6：按事务方向读取预备定义，批次外分组继续使用当前定义。
    pub(super) fn batch_generation(
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
pub(super) fn remount(old: &BatchInstance, next: &BatchInstance) -> bool {
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
pub(super) fn batch_inherited<'a>(
    transition: &'a lifecycle::Transition,
    before: &BatchIndex<'_>,
    target: &[BatchInstance],
    overrides: HashMap<String, Option<payload::State>>,
) -> Cow<'a, SharedStates> {
    let mut inherited = Cow::Borrowed(&transition.inherited);
    for item in target {
        if before.get(&item.specification.name).is_some_and(|old| {
            old.specification.plugin != item.specification.plugin
                || old.specification.component != item.specification.component
        }) {
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
