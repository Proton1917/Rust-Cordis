//! ADR-0052 I-1：按当前核心绑定取得事务的传递依赖方，统一交接最新状态。

use super::*;
use generation::{Binding, Configuration};

impl Manager {
    /// SW-5：分组和单库替换共用候选提供声明、实际绑定及状态冻结范围。
    pub(super) async fn prepare_replacement_transition(
        &self,
        group: &str,
        plugin: Option<&str>,
        candidate: &GroupGeneration,
    ) -> Result<lifecycle::Transition> {
        let instances: Vec<_> = self
            .group_instances(group)
            .into_iter()
            .filter(|instance| plugin.is_none_or(|name| instance.data.plugin == name))
            .collect();
        let incoming = instances.iter().filter_map(|instance| {
            candidate
                .components
                .get(&(
                    instance.data.plugin.clone(),
                    instance.data.component.clone(),
                ))
                .map(|component| (instance.fiber.context(), component.clone()))
        });
        let instances = self.include_incoming(instances.clone(), incoming);
        self.prepare_transition(&instances, true).await
    }

    /// SW-8：记录实际进入候选纪元的实例，包含批次新增实例。
    pub(super) fn begin_confirmation(&self, transition: &lifecycle::Transition) {
        let mut instances: Vec<_> = self
            .0
            .catalog
            .lock()
            .instances
            .iter()
            .filter(|(name, instance)| {
                instance.fiber.state() == cordis::FiberState::Active
                    && transition
                        .before
                        .get(*name)
                        .is_none_or(|before| *before != instance.fiber.epoch())
            })
            .map(|(name, _)| name.clone())
            .collect();
        instances.sort();
        *transition.confirming.lock() = Some((std::time::Instant::now(), instances));
    }

    /// ADR-0052：候选新增服务会重建同域中的可选依赖方，已有 Binding 继续决定当前边。
    pub(super) fn include_incoming(
        &self,
        mut roots: Vec<Arc<Instance>>,
        incoming: impl IntoIterator<Item = (Context, Arc<RemoteComponent>)>,
    ) -> Vec<Arc<Instance>> {
        let mut provided = HashSet::new();
        for (view, component) in incoming {
            for interface in &component.provides {
                provided.insert((interface.key.clone(), view.realm_named(&interface.key)));
            }
        }
        let catalog = self.0.catalog.lock().instances.clone();
        for instance in catalog.values() {
            let Some(current) = instance.data.current.lock().upgrade() else {
                continue;
            };
            if current.component.inject.iter().any(|(interface, _)| {
                provided.contains(&(
                    interface.key.clone(),
                    current.context.realm_named(&interface.key),
                )) && current
                    .context
                    .lookup_named(interface.key.clone())
                    .is_none()
            }) && !roots.iter().any(|root| Arc::ptr_eq(root, instance))
            {
                roots.push(instance.clone());
            }
        }
        roots
    }

    /// SW-5：保守纳入的实例沿原纪元继续运行时，归还其冻结写入权限。
    pub(super) async fn thaw_current(&self, frozen: &lifecycle::Frozen) -> Result<()> {
        let current = lifecycle::Frozen {
            held: frozen
                .held
                .iter()
                .filter(|(generation, _)| generation.is_current())
                .cloned()
                .collect(),
            ..Default::default()
        };
        self.thaw(&current).await
    }

    /// DOC-11 §7：宿主退出时从原启动计划恢复连接，仍存活的原进程沿用既有连接。
    pub(super) async fn rollback_version(
        &self,
        group: &Arc<Group>,
        previous: &Arc<GroupGeneration>,
    ) -> Result<Arc<GroupGeneration>> {
        if previous.connection.is_alive() {
            return Ok(previous.clone());
        }
        let specification = group
            .specification
            .lock()
            .clone()
            .ok_or_else(|| error(4, "原连接退出且缺少进程启动计划"))?;
        let (restored, plan) = self
            .prepare_launch(specification.specification.clone())
            .await?;
        self.replace_plan(group, Some(plan)).await;
        self.monitor(group.clone(), restored.clone())?;
        Ok(restored)
    }

    /// ADR-0053：计划更新只在锁内转交所有权，旧来源在锁外归还并提交有界回收。
    pub(super) async fn replace_plan(&self, group: &Group, next: Option<LaunchPlan>) {
        let previous = std::mem::replace(&mut *group.specification.lock(), next);
        let directories: std::collections::BTreeSet<_> = previous
            .iter()
            .flat_map(|plan| plan._pins.iter().map(|pin| pin.directory().to_owned()))
            .collect();
        drop(previous);
        if !directories.is_empty()
            && let Err(problem) = cordis_process::cache::prune_directories(
                directories.into_iter().collect(),
                self.0.options.cleanup_timeout,
            )
            .await
        {
            self.0
                .context
                .logger()
                .warn(format!("库副本回收失败：{problem}"));
        }
    }

    /// ADR-0052 R-2：候选确认窗口回滚时报告被撤销的实例及窗口时长。
    pub(super) fn confirmation_discard(
        &self,
        transition: &lifecycle::Transition,
    ) -> Option<cordis_types::ConfirmationDiscard> {
        let (started, instances) = transition.confirming.lock().clone()?;
        let loss = cordis_types::ConfirmationDiscard {
            instances,
            configured_ms: self.0.options.confirm.as_millis().min(u64::MAX as u128) as u64,
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        };
        let message = metadata::clipped(&format!(
            "确认期回滚丢弃内存状态：instances={:?} configured_ms={} elapsed_ms={}",
            loss.instances, loss.configured_ms, loss.elapsed_ms
        ));
        let sink = self.0.log_sink.lock().clone();
        if let Some(sink) = sink {
            sink(LogRecord {
                origin: LogOrigin::Protocol,
                connection: 0,
                context: 0,
                instance: "core".into(),
                level: 1,
                message,
            });
        } else {
            self.0.context.logger().warn(message);
        }
        Some(loss)
    }

    /// ADR-0052 I-1：配置变化和依赖方共用最新快照，加载失败时恢复原配置及原状态。
    pub(super) async fn update_stateful(
        &self,
        instance: Arc<Instance>,
        config: Value,
    ) -> Result<()> {
        let current = self.group(&instance.data.group)?.current.lock().clone();
        let component = current
            .components
            .get(&(
                instance.data.plugin.clone(),
                instance.data.component.clone(),
            ))
            .cloned()
            .ok_or_else(|| error(4, "配置目标组件缺席"))?;
        let roots = self.include_incoming(
            vec![instance.clone()],
            [(instance.fiber.context(), component)],
        );
        let transition = self.prepare_transition(&roots, true).await?;
        let previous = instance.data.config.lock().clone();
        self.inherit(&transition.inherited);
        let outcome = match self.set_configuration(&instance, config).await {
            Ok(()) => self
                .stabilize_with(&transition.before, &transition.names, true)
                .await
                .map(|_| ()),
            Err(problem) => Err(problem),
        };
        if let Err(problem) = outcome {
            self.inherit(&transition.inherited);
            let restored = async {
                self.set_configuration(&instance, previous).await?;
                self.stabilize_with(&transition.before, &transition.names, true)
                    .await
            }
            .await;
            let mut failure = problem;
            if let Err(restored) = restored {
                failure = error(
                    1,
                    format!(
                        "配置更新失败：{}；恢复失败：{}",
                        metadata::clipped(&failure.message),
                        metadata::clipped(&restored.message)
                    ),
                );
            }
            if let Err(thaw) = self.thaw_current(&transition.frozen).await {
                failure.message = format!(
                    "{}；解冻失败：{}",
                    metadata::clipped(&failure.message),
                    metadata::clipped(&thaw.message)
                );
            }
            return Err(failure);
        }
        self.thaw_current(&transition.frozen).await?;
        Ok(())
    }

    async fn set_configuration(&self, instance: &Instance, config: Value) -> Result<()> {
        *instance.data.config.lock() = config.clone();
        instance
            .fiber
            .update(Configuration {
                instance: instance.data.clone(),
                config,
            })
            .await
            .map_err(|problem| {
                instance
                    .data
                    .error
                    .lock()
                    .as_ref()
                    .map(|problem| problem.value.clone())
                    .unwrap_or_else(|| kernel_error(problem))
            })
    }

    /// 管理服务在应答前判断本次变更是否会撤回调用方或其依赖。
    pub fn change_affects(&self, caller: &str, target: &str, replace_library: bool) -> bool {
        if caller == target {
            return true;
        }
        let Ok(instance) = self.instance(target) else {
            return false;
        };
        let roots = if replace_library {
            self.group_instances(&instance.data.group)
                .into_iter()
                .filter(|item| item.data.plugin == instance.data.plugin)
                .collect()
        } else {
            vec![instance]
        };
        self.affected_instances(&roots)
            .iter()
            .any(|item| item.data.name == caller)
    }

    /// 当前 Binding 指明真实提供方，闭包沿反向边扩展；目录锁在读取绑定前释放。
    pub(super) fn affected_instances(&self, roots: &[Arc<Instance>]) -> Vec<Arc<Instance>> {
        let instances = self.0.catalog.lock().instances.clone();
        let mut dependents = HashMap::<String, Vec<String>>::new();
        for (name, instance) in &instances {
            let Some(current) = instance.data.current.lock().upgrade() else {
                continue;
            };
            for (interface, _) in &current.component.inject {
                let Ok(value) = current.context.get_named(interface.key.clone()) else {
                    continue;
                };
                if let Some(binding) = value.downcast_ref::<Binding>()
                    && let Some(provider) = &binding.generation
                {
                    dependents
                        .entry(provider.instance.name.clone())
                        .or_default()
                        .push(name.clone());
                }
            }
        }
        let mut names: HashSet<_> = roots
            .iter()
            .map(|instance| instance.data.name.clone())
            .collect();
        let mut pending: Vec<_> = names.iter().cloned().collect();
        while let Some(provider) = pending.pop() {
            for consumer in dependents.get(&provider).into_iter().flatten() {
                if names.insert(consumer.clone()) {
                    pending.push(consumer.clone());
                }
            }
        }
        let mut names: Vec<_> = names.into_iter().collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|name| instances.get(&name).cloned())
            .collect()
    }
}
