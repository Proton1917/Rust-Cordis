//! ADR-0031：防抖窗口中的配置和候选引用一次提交，失败恢复窗口前部署。

use super::*;
use cordis_supervisor::manager::{BatchInstance, BatchReplacement};

struct TargetInstance {
    settings: InstanceSettings,
    package: Package,
    group: String,
    view: Context,
    manage: Option<NamedProvision>,
    remount: bool,
}

struct TargetDeployment {
    configuration: Configuration,
    overlay: Overlay,
    instances: BTreeMap<String, TargetInstance>,
    groups: BTreeMap<String, Vec<Package>>,
    sources: Vec<BatchReplacement>,
}

impl Controller {
    /// ADR-0031：预备最终部署后调用唯一监管事务，叠加层在全批确认后发布。
    pub(super) async fn replace_batch(
        &mut self,
        replacements: &[(String, String)],
        configuration: Option<Configuration>,
    ) -> Result<()> {
        let before = self.batch_current_instances()?;
        let mut target = self
            .prepare_batch_target(replacements, configuration)
            .await?;
        let mut desired = match batch_instances(&target.instances) {
            Ok(desired) => desired,
            Err(problem) => {
                release_target(target).await?;
                return Err(problem);
            }
        };
        let sources = std::mem::take(&mut target.sources);
        let result = self
            .manager
            .replace_batch(sources, before.clone(), desired.clone(), HashMap::new())
            .await;
        if result.is_ok() || result.as_ref().is_err_and(replacement_committed) {
            let snapshot = self.manager.snapshot();
            for item in &mut desired {
                if let Some(actual) = snapshot
                    .instances
                    .iter()
                    .find(|actual| actual.name == item.specification.name)
                {
                    item.specification.component = actual.component.clone();
                }
            }
        }
        let states = match result {
            Ok(states) => states,
            Err(problem) if replacement_committed(&problem) => {
                let original = self.batch_original_states();
                return self
                    .restore_batch(target, before, desired, original, problem)
                    .await;
            }
            Err(problem) => {
                self.refresh_batch_fibers()?;
                release_target(target).await?;
                return Err(problem);
            }
        };
        if let Err(problem) = self.persist_batch(&target, &states).await {
            return self
                .restore_batch(target, before, desired, states, problem)
                .await;
        }
        self.accept_batch_target(target).await?;
        self.store.retain_replacements(&self.overlay)
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

    /// loader EntryGroup.update、ADR-0031：最终配置先展开，再把窗口候选映射到完整实例集合。
    async fn prepare_batch_target(
        &mut self,
        replacements: &[(String, String)],
        configuration: Option<Configuration>,
    ) -> Result<TargetDeployment> {
        let candidates = self.batch_candidates(replacements).await?;
        if configuration.is_some() {
            self.discover().await?;
        }
        let mut configuration = configuration.unwrap_or_else(|| self.configuration.clone());
        if configuration.runtime != self.configuration.runtime {
            self.logs
                .emit(1, "runtime", "runtime设置将在下次启动时生效");
            configuration.runtime = self.configuration.runtime.clone();
        }
        let mut target = TargetDeployment {
            configuration,
            overlay: self.overlay.clone(),
            instances: BTreeMap::new(),
            groups: BTreeMap::new(),
            sources: Vec::new(),
        };
        self.sources
            .lock()
            .apply_declarations(&target.configuration, &mut target.overlay);
        let settings = target.configuration.effective(&target.overlay)?;
        let result = async {
            for (name, settings) in settings {
                self.prepare_batch_instance(&mut target, &candidates, name, settings)
                    .await?;
            }
            target.sources = self.batch_sources(&self.groups, &target.groups)?;
            Ok(())
        }
        .await;
        if let Err(problem) = result {
            release_target(target).await?;
            return Err(problem);
        }
        Ok(target)
    }

    /// ADR-0025、ADR-0031：上下文变化建立新视图，其他实例继续使用其原始隔离域身份。
    async fn prepare_batch_instance(
        &mut self,
        target: &mut TargetDeployment,
        candidates: &BTreeMap<(String, String), Package>,
        name: String,
        mut settings: InstanceSettings,
    ) -> Result<()> {
        let declared = target
            .configuration
            .instance
            .get(&name)
            .map(|item| item.plugin.as_str())
            .unwrap_or(&settings.plugin);
        let reference = self
            .sources
            .lock()
            .select(&name, declared, &settings.plugin);
        let original = self.installed_package(&reference).await?;
        let group = settings
            .group
            .clone()
            .unwrap_or_else(|| original.name.clone());
        config::instance_name(&group)?;
        let package = candidates
            .get(&(group.clone(), original.name.clone()))
            .cloned()
            .unwrap_or(original);
        if candidates.contains_key(&(group.clone(), package.name.clone())) {
            self.record_batch_version(target, &name, &package);
            settings.plugin = package.reference.clone();
        }
        let packages = target.groups.entry(group.clone()).or_default();
        if let Some(previous) = packages.iter().find(|item| item.name == package.name) {
            if previous.reference != package.reference {
                return Err(parameter("最终分组包含同逻辑插件的不同来源"));
            }
        } else {
            packages.push(package.clone());
        }
        let old = self.mounted.get(&name);
        let remount = old.is_none_or(|old| {
            old.group != group
                || old.package.name != package.name
                || old.settings.component != settings.component
                || old.settings.isolate != settings.isolate
                || old.settings.intercept != settings.intercept
                || old.settings.grants != settings.grants
        });
        let view = if remount {
            self.mount_view(&settings)?
        } else {
            old.expect("existing view")._view.clone()
        };
        let manage = if remount {
            self.provide_management(&name, &settings, &view)?
        } else {
            None
        };
        target.instances.insert(
            name,
            TargetInstance {
                settings,
                package,
                group,
                view,
                manage,
                remount,
            },
        );
        Ok(())
    }

    /// DOC-15 §3.2、DOC-11 §7：保存原始成员条目及实际装载引用，继承的scope信息继续由配置展开。
    fn record_batch_version(&self, target: &mut TargetDeployment, name: &str, package: &Package) {
        let raw = target.configuration.entries(&target.overlay);
        let mut entry = OverlayEntry::from(&raw[name]);
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
        self.mounted
            .iter()
            .map(|(name, item)| {
                let actual = snapshot
                    .instances
                    .iter()
                    .find(|instance| instance.name == *name)
                    .ok_or_else(|| WireError::new(4, "运行目录缺少已挂载实例"))?;
                Ok(BatchInstance {
                    specification: InstanceSpec {
                        name: name.clone(),
                        group: item.group.clone(),
                        plugin: item.package.name.clone(),
                        component: actual.component.clone(),
                        config: Value::serialized(&item.settings.config).map_err(parameter)?,
                    },
                    version: item.package.version.clone(),
                    view: item._view.clone(),
                    remount: false,
                })
            })
            .collect()
    }

    /// ADR-0031、AC-2：成员集合相同的原生组只准备变化库，进程插件或组结构变化准备进程。
    fn batch_sources(
        &self,
        current: &BTreeMap<String, Vec<Package>>,
        target: &BTreeMap<String, Vec<Package>>,
    ) -> Result<Vec<BatchReplacement>> {
        let mut replacements = Vec::new();
        for (group, packages) in target {
            let previous = current.get(group);
            let native = packages
                .iter()
                .all(|package| matches!(package.entry, Entry::Native { .. }))
                && previous.is_some_and(|old| {
                    old.len() == packages.len()
                        && old
                            .iter()
                            .all(|item| packages.iter().any(|package| package.name == item.name))
                });
            if native {
                for package in packages {
                    if previous.is_some_and(|old| {
                        old.iter().any(|item| item.reference == package.reference)
                    }) {
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
                        .any(|package| !old.iter().any(|item| item.reference == package.reference))
            }) {
                replacements.push(BatchReplacement::Process(catalog::plan(
                    group,
                    packages,
                    &self.project,
                    &self.configuration.runtime,
                    &self.locations,
                )?));
            }
        }
        Ok(replacements)
    }

    /// DOC-15 §3.2、CP-11：原快照和当前导出状态成功写入后一次发布最终叠加层。
    async fn persist_batch(
        &self,
        target: &TargetDeployment,
        original: &HashMap<String, Option<payload::State>>,
    ) -> Result<()> {
        for (name, previous) in &target.overlay.previous {
            if target
                .instances
                .get(name)
                .is_some_and(|item| item.package.reference != previous.plugin)
                && let Some(state) = original.get(name)
            {
                self.store
                    .save_replacement(name, &previous.plugin, state.as_ref())?;
            }
        }
        let exported = self.manager.checkpoint().await?;
        let states = target
            .instances
            .keys()
            .map(|name| (name.clone(), exported.get(name).cloned()))
            .collect();
        self.store.restore_states(&states)?;
        for name in self
            .mounted
            .keys()
            .filter(|name| !target.instances.contains_key(*name))
        {
            self.store.forget(name)?;
        }
        self.store.save_overlay(&target.overlay)
    }

    /// DOC-11 §7：提交后清理或持久化失败时重新准备原来源，恢复整个窗口的配置和状态。
    async fn restore_batch(
        &mut self,
        target: TargetDeployment,
        before: Vec<BatchInstance>,
        desired: Vec<BatchInstance>,
        original: HashMap<String, Option<payload::State>>,
        problem: WireError,
    ) -> Result<()> {
        let sources = self.batch_sources(&target.groups, &self.groups)?;
        let restored = self
            .manager
            .replace_batch(sources, desired, before, original.clone())
            .await;
        let original_active =
            restored.is_ok() || restored.as_ref().is_err_and(replacement_committed);
        if original_active {
            self.refresh_batch_fibers()?;
            self.store.restore_states(&original)?;
            release_target(target).await?;
            self.store.retain_replacements(&self.overlay)?;
        } else {
            self.accept_batch_target(target).await?;
            self.store.save_overlay(&self.overlay)?;
        }
        let mut failure = match restored {
            Ok(_) => problem,
            Err(restored) => {
                WireError::new(1, format!("批次失败：{problem}；恢复结果：{restored}"))
            }
        };
        failure.detail = Value::serialized(&payload::ReplacementFailure {
            group: "batch".into(),
            plugin: None,
            committed: !original_active,
            cause: failure.detail,
        })
        .map_err(parameter)?;
        Err(failure)
    }

    /// ADR-0031：已提交错误从监管器取回原快照，覆盖配置移除和改变分组的实例。
    fn batch_original_states(&self) -> HashMap<String, Option<payload::State>> {
        self.groups
            .keys()
            .flat_map(|group| self.manager.last_replacement_states(group))
            .collect()
    }

    /// ADR-0031：恢复后的纤程句柄来自监管器当前目录，配置和授权仍为窗口前值。
    fn refresh_batch_fibers(&mut self) -> Result<()> {
        for (name, item) in &mut self.mounted {
            item.fiber = self.manager.instance_fiber(name)?;
        }
        Ok(())
    }

    /// DOC-15 §3.2：确认后替换挂载记录并释放被取代授权，保留相同视图的已有授权对象。
    async fn accept_batch_target(&mut self, target: TargetDeployment) -> Result<()> {
        let mut prior = std::mem::take(&mut self.mounted);
        for (name, item) in target.instances {
            let old = prior.remove(&name);
            let manage = if item.remount {
                if let Some(old) = old
                    && let Some(grant) = old.manage
                {
                    grant.dispose().await.map_err(kernel)?;
                }
                item.manage
            } else {
                old.and_then(|old| old.manage)
            };
            self.mounted.insert(
                name.clone(),
                Mounted {
                    settings: item.settings,
                    package: item.package,
                    group: item.group,
                    fiber: self.manager.instance_fiber(&name)?,
                    _view: item.view,
                    manage,
                },
            );
        }
        for (_, old) in prior {
            if let Some(grant) = old.manage {
                grant.dispose().await.map_err(kernel)?;
            }
        }
        self.configuration = target.configuration;
        self.overlay = target.overlay;
        self.groups = target.groups;
        *self.permissions.lock() = self.configuration.effective(&self.overlay)?;
        self.refresh()
    }
}

/// ADR-0031：空组件名由监管器基于候选描述符按单组件规则选择。
fn batch_instances(instances: &BTreeMap<String, TargetInstance>) -> Result<Vec<BatchInstance>> {
    instances
        .iter()
        .map(|(name, item)| {
            Ok(BatchInstance {
                specification: InstanceSpec {
                    name: name.clone(),
                    group: item.group.clone(),
                    plugin: item.package.name.clone(),
                    component: item.settings.component.clone().unwrap_or_default(),
                    config: Value::serialized(&item.settings.config).map_err(parameter)?,
                },
                version: item.package.version.clone(),
                view: item.view.clone(),
                remount: item.remount,
            })
        })
        .collect()
}

/// ADR-0031、T-EFF-2：候选部署退出时释放其新建管理能力，原实例授权继续存活。
async fn release_target(target: TargetDeployment) -> Result<()> {
    for item in target.instances.into_values() {
        if let Some(grant) = item.manage {
            grant.dispose().await.map_err(kernel)?;
        }
    }
    Ok(())
}
