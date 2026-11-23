//! ADR-0031：防抖窗口中的配置和候选引用一次提交，失败恢复窗口前部署。

use super::*;
use cordis_supervisor::manager::{BatchInstance, BatchReplacement};
use std::collections::BTreeSet;

pub(super) struct TargetDeployment {
    configuration: Configuration,
    overlay: Overlay,
    instances: BTreeMap<String, Mounted>,
    // 已启动宿主可能继续持有暂时无实例的库，直到整组退役。
    groups: BTreeMap<String, Vec<Package>>,
    group_references: HashMap<(String, String), String>,
    migrated: bool,
}

pub(super) struct PendingTarget {
    _target: TargetDeployment,
    _pages: Vec<web::WebChange>,
}

impl Controller {
    pub(super) async fn start_target(&mut self) -> Result<()> {
        let target = self
            .prepare_batch_target(&[], None, self.overlay.clone(), true)
            .await?;
        self.overlay = target.overlay;
        for (group, packages) in target.groups {
            if let Err(problem) = self.attach_start_group(&group, packages).await {
                for (name, item) in &target.instances {
                    if item.group == group && !matches!(item.package.entry, Entry::Web) {
                        self.instance_error(name, &problem);
                    }
                }
            }
        }
        for (name, item) in target.instances {
            let failed = self.diagnostics.lock().instance_errors.contains_key(&name);
            let result = if failed {
                item.release().await
            } else {
                self.mount_target_instance(&name, item, self.store.load(&name))
                    .await
            };
            if let Err(problem) = result {
                self.instance_error(&name, &problem);
            }
        }
        self.refresh()?;
        if target.migrated || self.store.accepted_base()?.is_none() {
            self.sources.observe(&InstanceOwners {
                mounted: &self.mounted,
                manager: &self.manager,
                web: &self.web,
            });
            let decision = self.sources.commit(
                &self.store,
                &self.configuration,
                &self.overlay,
                &HashMap::new(),
            );
            self.record_decision(decision)?;
        }
        Ok(())
    }

    async fn attach_start_group(&mut self, group: &str, packages: Vec<Package>) -> Result<()> {
        self.manager
            .attach_group(self.group_plan(group, &packages)?)
            .await?;
        self.groups.insert(group.into(), packages);
        Ok(())
    }

    /// 套件恢复沿用冷启动注册，下一阶段由契约会话等待激活。
    pub(super) async fn resume_contract_suite(&mut self, name: &str) -> Result<()> {
        let mut target = self
            .prepare_batch_target(&[], None, self.overlay.clone(), false)
            .await?;
        let item = target
            .instances
            .remove(name)
            .ok_or_else(|| parameter("部署目标缺少验证套件"))?;
        if let Some(packages) = target.groups.remove(&item.group)
            && !self.groups.contains_key(&item.group)
        {
            self.attach_start_group(&item.group, packages).await?;
        }
        Arc::make_mut(&mut self.diagnostics.lock().desired)
            .insert(name.into(), item.settings.clone());
        self.mount_target_instance(name, item, Ok(None)).await?;
        self.refresh()
    }

    async fn mount_target_instance(
        &mut self,
        name: &str,
        item: Mounted,
        state: Result<Option<payload::State>>,
    ) -> Result<()> {
        let result = async {
            let inherited = state?;
            let spec = item
                .package
                .web
                .as_ref()
                .map(|_| Self::web_spec(name, &item.settings, &item.package, &item.scope.view))
                .transpose()?;
            let backend = !matches!(item.package.entry, Entry::Web);
            if backend {
                self.manager
                    .restore_instance(
                        item.registration(name, item.settings.component.as_deref(), true)?,
                        inherited,
                    )
                    .await?;
            }
            if let Some(spec) = spec
                && let Err(problem) = self.web.mount(spec)
            {
                if backend && let Err(cleanup) = self.manager.dispose(name).await {
                    return Err(web::recovery_error("实例挂载", problem, cleanup));
                }
                return Err(problem);
            }
            Ok(())
        }
        .await;
        if let Err(problem) = result {
            if let Err(cleanup) = item.release().await {
                return Err(web::recovery_error("挂载授权", problem, cleanup));
            }
            return Err(problem);
        }
        self.mounted.insert(name.into(), item);
        self.diagnostics.lock().instance_errors.remove(name);
        Ok(())
    }

    /// ADR-0031：预备最终部署后调用唯一监管事务，叠加层在全批确认后发布。
    pub(super) async fn replace_batch(
        &mut self,
        replacements: &[(String, String)],
        configuration: Option<Configuration>,
    ) -> Result<()> {
        self.apply_deployment_batch(replacements, configuration, self.overlay.clone(), true)
            .await
    }

    /// ADR-0031、DOC-15 §11.4：配置、后端与网页版本统一准备；失败归还新建授权。
    pub(super) async fn apply_deployment_batch(
        &mut self,
        replacements: &[(String, String)],
        configuration: Option<Configuration>,
        overlay: Overlay,
        durable: bool,
    ) -> Result<()> {
        let reconfigure = configuration.is_some() || !durable;
        let before = self.batch_current_instances()?;
        let mut target = self
            .prepare_batch_target(replacements, configuration, overlay, false)
            .await?;
        if !durable {
            for (group, packages) in &mut target.groups {
                if let Some(active) = self.groups.get(group) {
                    *packages = active.clone();
                }
            }
        }
        let sources = match self.batch_sources(&target.groups) {
            Ok(sources) => sources,
            Err(problem) => {
                release_mounts(target.instances).await?;
                return Err(problem);
            }
        };
        if sources.is_empty()
            && target.configuration == self.configuration
            && target.overlay == self.overlay
            && target.instances.len() == self.mounted.len()
            && target.instances.iter().all(|(name, item)| {
                self.mounted.get(name).is_some_and(|old| {
                    old.settings == item.settings
                        && old.package.reference == item.package.reference
                        && Arc::ptr_eq(&old.scope, &item.scope)
                }) && self.instance_fiber(name).is_ok_and(|fiber| {
                    fiber.is_settled()
                        && matches!(
                            fiber.state(),
                            cordis::FiberState::Active | cordis::FiberState::Pending
                        )
                })
            })
        {
            return Ok(());
        }
        let desired = match self.planned_instances(&target.instances) {
            Ok(desired) => desired,
            Err(problem) => {
                release_mounts(target.instances).await?;
                return Err(problem);
            }
        };
        let pages = match self.prepare_batch_pages(&target).await {
            Ok(pages) => pages,
            Err(web::PreparationFailure::Rejected(problem)) => {
                release_mounts(target.instances).await?;
                return Err(problem);
            }
            Err(web::PreparationFailure::RecoveryRequired(problem, pages)) => {
                self.held_target = Some(PendingTarget {
                    _target: target,
                    _pages: pages,
                });
                return self.require_recovery(problem);
            }
        };
        self.diagnostics.lock().candidate_forwarders = Some(Arc::new(
            target
                .instances
                .iter()
                .filter(|(_, item)| item.settings.forwarder)
                .map(|(name, _)| name.clone())
                .collect(),
        ));
        self.execute_batch_target(
            target,
            sources,
            before,
            desired,
            pages,
            reconfigure,
            durable,
        )
        .await
    }

    /// SW-5–SW-9：后端先稳定，再确认每个页面，持久化成功后发布新运行目录。
    async fn execute_batch_target(
        &mut self,
        target: TargetDeployment,
        sources: Vec<BatchReplacement>,
        before: Vec<BatchInstance>,
        desired: Vec<BatchInstance>,
        pages: Vec<web::WebChange>,
        reconfigure: bool,
        durable: bool,
    ) -> Result<()> {
        use cordis_supervisor::manager::{BatchFailure, DeploymentDecision};
        let manager = self.manager.clone();
        let candidate = &target;
        let changes = &pages;
        let controller = &mut *self;
        let completion = manager
            .deploy(
                sources,
                before,
                desired,
                reconfigure,
                move |states| async move {
                    let finalized = match controller.finish_web_changes(changes).await {
                        Ok(()) if durable => controller.persist_batch(candidate, &states),
                        Ok(()) => Ok(()),
                        Err(problem) => Err(problem),
                    };
                    match finalized {
                        Ok(()) => DeploymentDecision::Committed,
                        Err(problem) if controller.recovery_required.is_some() => {
                            DeploymentDecision::RecoveryRequired(problem)
                        }
                        Err(problem) => match controller.restore_web_configurations(changes) {
                            Ok(()) => DeploymentDecision::NotCommitted(problem),
                            Err(recovery) => DeploymentDecision::RecoveryRequired(
                                web::recovery_error("页面", problem, recovery),
                            ),
                        },
                    }
                },
            )
            .await;
        match completion {
            Ok(commit) => {
                self.accept_batch_target(target, durable).await?;
                if let Some(problem) = commit.cleanup {
                    self.report_cleanup("原部署资源", problem);
                }
                Ok(())
            }
            Err(BatchFailure::RecoveryRequired(problem)) => {
                self.held_target = Some(PendingTarget {
                    _target: target,
                    _pages: pages,
                });
                self.require_recovery(problem)
            }
            Err(failure) => {
                let problem = match failure {
                    BatchFailure::Rejected(problem) | BatchFailure::RecoveryRequired(problem) => {
                        problem
                    }
                    BatchFailure::Restored { problem, discarded } => {
                        self.batch_restored = Some(discarded);
                        problem
                    }
                };
                self.diagnostics.lock().candidate_forwarders = None;
                if let Err(recovery) = self.rollback_web_changes(&pages).await {
                    self.held_target = Some(PendingTarget {
                        _target: target,
                        _pages: pages,
                    });
                    return self.require_recovery(web::recovery_error("批次", problem, recovery));
                }
                if let Err(cleanup) = release_mounts(target.instances).await {
                    return self.require_recovery(web::recovery_error(
                        "批次授权",
                        problem,
                        cleanup,
                    ));
                }
                Err(problem)
            }
        }
    }

    /// ADR-0031：候选按窗口前实例身份定位，代表实例移除后继续作用于同组同逻辑插件。
    async fn batch_candidates(
        &mut self,
        replacements: &[(String, String)],
    ) -> Result<BTreeMap<(String, String), Package>> {
        let mut candidates = BTreeMap::new();
        for (name, reference) in replacements {
            let candidate = self.installed_package(reference).await?;
            let current = self
                .mounted
                .get(name)
                .ok_or_else(|| WireError::new(4, format!("批次原实例 {name} 缺席")))?;
            if candidate.name != current.package.name {
                return Err(parameter("批次替换需要保持逻辑插件名称"));
            }
            if candidate.reference == current.package.reference {
                continue;
            }
            let key = (current.group.clone(), candidate.name.clone());
            if candidates
                .get(&key)
                .is_some_and(|previous: &Package| previous.reference != candidate.reference)
            {
                return Err(parameter("同一批次对相同逻辑插件提供了不同候选"));
            }
            candidates.insert(key, candidate);
        }
        Ok(candidates)
    }

    /// DOC-15 §11.4：网页变更覆盖新增、移除、视图与版本变化，后端描述由实际激活补齐。
    async fn prepare_batch_pages(
        &self,
        target: &TargetDeployment,
    ) -> std::result::Result<Vec<web::WebChange>, web::PreparationFailure> {
        let names = self
            .mounted
            .keys()
            .chain(target.instances.keys())
            .collect::<BTreeSet<_>>();
        let mut candidates = Vec::new();
        for name in names {
            let old = self.mounted.get(name);
            let next = target.instances.get(name);
            if old.zip(next).is_some_and(|(old, next)| {
                old.settings == next.settings
                    && old.package.reference == next.package.reference
                    && Arc::ptr_eq(&old.scope, &next.scope)
            }) {
                continue;
            }
            let specification = next
                .filter(|item| item.package.web.is_some())
                .map(|item| Self::web_spec(name, &item.settings, &item.package, &item.scope.view))
                .transpose()?;
            candidates.push((
                name.clone(),
                specification,
                next.is_some_and(|item| item.remount(old)),
            ));
        }
        self.prepare_web_targets(candidates).await
    }

    /// loader EntryGroup.update、ADR-0031：最终配置先展开，再把窗口候选映射到完整实例集合。
    async fn prepare_batch_target(
        &mut self,
        replacements: &[(String, String)],
        configuration: Option<Configuration>,
        overlay: Overlay,
        startup: bool,
    ) -> Result<TargetDeployment> {
        if configuration.is_some() {
            self.discover().await?;
        }
        let candidates = self.batch_candidates(replacements).await?;
        let configuration = configuration.unwrap_or_else(|| self.configuration.clone());
        if configuration.runtime != self.runtime {
            self.logs
                .emit(1, "runtime", "runtime设置将在下次启动时生效");
        }
        let mut target = TargetDeployment {
            configuration,
            overlay,
            instances: BTreeMap::new(),
            groups: BTreeMap::new(),
            group_references: HashMap::new(),
            migrated: false,
        };
        target.migrated = self
            .sources
            .apply_declarations(&target.configuration, &mut target.overlay);
        let settings = target.configuration.effective(&target.overlay)?;
        if startup {
            self.diagnostics.lock().desired = Arc::new(settings.clone());
        }
        let result = async {
            for (name, settings) in settings {
                if let Err(problem) = self
                    .prepare_batch_instance(&mut target, &candidates, &name, settings)
                    .await
                {
                    if startup {
                        self.instance_error(&name, &problem);
                    } else {
                        return Err(problem);
                    }
                }
            }
            Ok(())
        }
        .await;
        if let Err(problem) = result {
            release_mounts(target.instances).await?;
            return Err(problem);
        }
        Ok(target)
    }

    /// ADR-0025、ADR-0031：上下文变化建立新视图，其他实例继续使用其原始隔离域身份。
    async fn prepare_batch_instance(
        &mut self,
        target: &mut TargetDeployment,
        candidates: &BTreeMap<(String, String), Package>,
        name: &str,
        mut settings: InstanceSettings,
    ) -> Result<()> {
        let (package, group) = self
            .batch_package(target, candidates, name, &mut settings)
            .await?;
        let item = self.target_instance(name, settings, package, group)?;
        target.instances.insert(name.into(), item);
        Ok(())
    }

    fn target_instance(
        &self,
        name: &str,
        settings: InstanceSettings,
        package: Package,
        group: String,
    ) -> Result<Mounted> {
        let old = self.mounted.get(name);
        let remount = old.is_none_or(|old| {
            old.group != group
                || old.package.name != package.name
                || matches!(old.package.entry, Entry::Web) != matches!(package.entry, Entry::Web)
                || old.settings.component != settings.component
                || old.settings.isolate != settings.isolate
                || old.settings.intercept != settings.intercept
                || old.settings.grants != settings.grants
                || old.settings.forwarder != settings.forwarder
        });
        let scope = if remount {
            let view = self.mount_view(&settings)?;
            let manage = self.provide_management(&settings, &view)?;
            Arc::new(MountScope { view, manage })
        } else {
            old.expect("existing view").scope.clone()
        };
        Ok(Mounted {
            settings,
            package,
            group,
            scope,
        })
    }

    /// ADR-0031：解析实例来源并形成目标宿主分组，网页 Owner 保持页面生命周期归属。
    async fn batch_package(
        &mut self,
        target: &mut TargetDeployment,
        candidates: &BTreeMap<(String, String), Package>,
        name: &str,
        settings: &mut InstanceSettings,
    ) -> Result<(Package, String)> {
        let declared = target
            .configuration
            .instance
            .get(name)
            .map(|item| item.plugin.as_str())
            .unwrap_or(&settings.plugin);
        let reference = self.sources.select(name, declared, &settings.plugin);
        let original = self.installed_package(&reference).await?;
        let group = original.group(
            settings.group.as_deref(),
            target.configuration.format_version,
        );
        config::instance_name(&group)?;
        let package = candidates
            .get(&(group.clone(), original.name.clone()))
            .cloned()
            .unwrap_or(original);
        if candidates.contains_key(&(group.clone(), package.name.clone())) {
            self.record_batch_version(target, name, &package);
            settings.plugin = package.reference.clone();
        }
        if !matches!(package.entry, Entry::Web) {
            let key = (group.clone(), package.name.clone());
            if let Some(previous) = target.group_references.get(&key) {
                if previous != &package.reference {
                    return Err(parameter("最终分组包含同逻辑插件的不同来源"));
                }
            } else {
                target
                    .group_references
                    .insert(key, package.reference.clone());
                target
                    .groups
                    .entry(group.clone())
                    .or_default()
                    .push(package.clone());
            }
        }
        Ok((package, group))
    }

    /// DOC-15 §3.2、DOC-11 §7：保存原始成员条目及实际装载引用，继承的scope信息继续由配置展开。
    fn record_batch_version(&self, target: &mut TargetDeployment, name: &str, package: &Package) {
        let raw = target
            .configuration
            .entry(&target.overlay, name)
            .expect("target member");
        let mut entry = OverlayEntry::from(&raw);
        entry.plugin = package.reference.clone();
        target.overlay.instance.insert(name.into(), entry);
        if let Some(previous) = self.mounted.get(name) {
            let raw = self.configuration.entries(&self.overlay);
            let mut previous_entry = OverlayEntry::from(&raw[name]);
            previous_entry.plugin = previous.package.reference.clone();
            target.overlay.previous.insert(name.into(), previous_entry);
        }
    }

    /// ADR-0031：已有实例使用实际组件名和原视图，供批次失败时恢复相同部署。
    fn batch_current_instances(&self) -> Result<Vec<BatchInstance>> {
        let snapshot = self.manager.snapshot();
        let actual: HashMap<_, _> = snapshot
            .instances
            .iter()
            .map(|instance| (instance.name.as_str(), instance))
            .collect();
        self.mounted
            .iter()
            .filter(|(_, item)| !matches!(item.package.entry, Entry::Web))
            .map(|(name, item)| {
                let actual = actual
                    .get(name.as_str())
                    .ok_or_else(|| WireError::new(4, "运行目录缺少已挂载实例"))?;
                item.registration(name, Some(&actual.component), false)
            })
            .collect()
    }

    fn planned_instances(
        &self,
        instances: &BTreeMap<String, Mounted>,
    ) -> Result<Vec<BatchInstance>> {
        instances
            .iter()
            .filter(|(_, item)| !matches!(item.package.entry, Entry::Web))
            .map(|(name, item)| {
                item.registration(
                    name,
                    item.settings.component.as_deref(),
                    item.remount(self.mounted.get(name)),
                )
            })
            .collect()
    }

    /// ADR-0031、AC-2：成员集合相同的原生组只准备变化库，进程插件或组结构变化准备进程。
    fn batch_sources(
        &self,
        target: &BTreeMap<String, Vec<Package>>,
    ) -> Result<Vec<BatchReplacement>> {
        let mut replacements = Vec::new();
        for (group, packages) in target {
            let previous = self.groups.get(group);
            let old_by_name: HashMap<_, _> = previous
                .into_iter()
                .flatten()
                .map(|package| (package.name.as_str(), package))
                .collect();
            let old_references: BTreeSet<_> = previous
                .into_iter()
                .flatten()
                .map(|package| package.reference.as_str())
                .collect();
            let native = previous.is_some_and(|old| old.len() == packages.len())
                && packages.iter().all(|package| {
                    matches!(package.entry, Entry::Native { .. })
                        && old_by_name
                            .get(package.name.as_str())
                            .is_some_and(|old| matches!(old.entry, Entry::Native { .. }))
                });
            if native {
                for package in packages {
                    if old_references.contains(package.reference.as_str()) {
                        continue;
                    }
                    let Entry::Native { library } = &package.entry else {
                        unreachable!()
                    };
                    replacements.push(BatchReplacement::Native {
                        group: group.clone(),
                        plugin: package.name.clone(),
                        path: library.clone(),
                    });
                }
            } else if previous.is_none_or(|old| {
                old.len() != packages.len()
                    || packages
                        .iter()
                        .any(|package| !old_references.contains(package.reference.as_str()))
            }) {
                replacements.push(BatchReplacement::Process(self.group_plan(group, packages)?));
            }
        }
        Ok(replacements)
    }

    /// 监管器仍持有原部署时写入候选的配置、状态与来源。
    fn persist_batch(
        &mut self,
        target: &TargetDeployment,
        states: &HashMap<String, Option<Arc<payload::State>>>,
    ) -> Result<()> {
        let mut next = self.sources.clone();
        next.track(
            &target.configuration,
            &target.overlay,
            target
                .instances
                .iter()
                .map(|(name, item)| (name, &item.settings, &item.package)),
        );
        next.observe(&InstanceOwners {
            mounted: &target.instances,
            manager: &self.manager,
            web: &self.web,
        });
        let decision = next.commit(&self.store, &target.configuration, &target.overlay, states);
        self.record_decision(decision)?;
        self.sources = next;
        Ok(())
    }

    /// DOC-15 §3.2：确认后替换挂载记录并释放被取代授权，保留相同视图的已有授权对象。
    async fn accept_batch_target(&mut self, target: TargetDeployment, durable: bool) -> Result<()> {
        let prior = std::mem::replace(&mut self.mounted, target.instances);
        self.configuration = target.configuration;
        if durable {
            self.overlay = target.overlay;
        }
        self.groups = target.groups;
        {
            let mut diagnostics = self.diagnostics.lock();
            diagnostics.desired = Arc::new(
                self.mounted
                    .iter()
                    .map(|(name, item)| (name.clone(), item.settings.clone()))
                    .collect(),
            );
            diagnostics.candidate_forwarders = None;
            diagnostics.instance_errors.clear();
        }
        self.refresh()?;
        if let Err(problem) = release_mounts(prior).await {
            self.report_cleanup("原授权", problem);
        }
        Ok(())
    }
}

impl Mounted {
    fn remount(&self, old: Option<&Self>) -> bool {
        old.is_none_or(|old| !Arc::ptr_eq(&old.scope, &self.scope))
    }

    /// 空组件名由监管器根据握手描述符计算。
    fn registration(
        &self,
        name: &str,
        component: Option<&str>,
        remount: bool,
    ) -> Result<BatchInstance> {
        Ok(BatchInstance {
            specification: InstanceSpec {
                name: name.into(),
                group: self.group.clone(),
                plugin: self.package.name.clone(),
                component: component.unwrap_or_default().into(),
                config: Value::serialized(&self.settings.config).map_err(parameter)?,
            },
            version: self.package.version.clone(),
            view: self.scope.view.clone(),
            remount,
        })
    }

    pub(super) async fn release(self) -> Result<()> {
        if let Some(grant) = Arc::into_inner(self.scope).and_then(|scope| scope.manage) {
            grant.dispose().await.map_err(kernel)?;
        }
        Ok(())
    }
}

/// ADR-0031、T-EFF-2：候选部署退出时释放其新建管理能力，原实例授权继续存活。
async fn release_mounts(instances: BTreeMap<String, Mounted>) -> Result<()> {
    let mut result = Ok(());
    for item in instances.into_values() {
        result = result.and(item.release().await);
    }
    result
}
