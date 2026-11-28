//! ADR-0052 I-1：按当前核心绑定取得事务的传递依赖方，统一交接最新状态。

use super::*;
use generation::{Binding, Configuration};

impl Manager {
    /// SW-8：归还继续运行的原实例写权，记录候选纪元，再观察本次变更的所有连接。
    pub(super) async fn confirm_transition(
        &self,
        transition: &mut lifecycle::Transition,
        changed: &[Fiber],
        connections: &[Connection],
    ) -> Result<Duration> {
        self.thaw_current(&transition.frozen).await?;
        let confirming = std::time::Instant::now();
        let mut instances: Vec<_> = self
            .0
            .catalog
            .lock()
            .instances
            .iter()
            .filter(|(name, instance)| {
                changed.contains(&instance.fiber)
                    && instance.fiber.state() == cordis::FiberState::Active
                    && transition
                        .before
                        .get(*name)
                        .is_none_or(|before| *before != instance.fiber.epoch())
            })
            .map(|(name, _)| name.clone())
            .collect();
        instances.sort();
        transition.confirming = Some((std::time::Instant::now(), instances));
        self.confirm(changed, connections).await?;
        Ok(confirming.elapsed())
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
        let mut directories: std::collections::BTreeSet<_> = previous
            .iter()
            .flat_map(|plan| plan._pins.iter().map(|pin| pin.directory().to_owned()))
            .collect();
        // Windows 宿主在退出前无法删除自身仍映射的 DLL，宿主退出后由核心一并回收其缓存目录。
        if let Some(plan) = &previous {
            let mut args = plan.specification.command.iter();
            while let Some(arg) = args.next() {
                if arg == "--cache-dir"
                    && let Some(dir) = args.next()
                {
                    let path = std::path::Path::new(dir);
                    if path.is_dir() {
                        directories.insert(path.to_owned());
                    }
                }
            }
        }
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
        let (started, instances) = transition.confirming.as_ref()?;
        let loss = cordis_types::ConfirmationDiscard {
            instances: instances.clone(),
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

    /// 按同一快照闭合真实绑定、父子关系和候选可能补足的依赖。
    pub(super) fn select_instances(
        &self,
        roots: impl IntoIterator<Item = String>,
        incoming: Vec<(Context, Arc<RemoteComponent>, String)>,
    ) -> Vec<Arc<Instance>> {
        let (instances, groups) = {
            let catalog = self.0.catalog.lock();
            (catalog.instances.clone(), catalog.groups.clone())
        };
        let mut dependents = HashMap::<String, Vec<String>>::new();
        let mut waiting = HashMap::<String, Vec<(cordis::Realm, u32, String)>>::new();
        let replaced: HashSet<_> = incoming.iter().map(|(_, _, name)| name.clone()).collect();
        let mut providers = incoming;
        for (name, instance) in &instances {
            let mut parent = instance.fiber.parent_context();
            while let Some(context) = parent {
                let fiber = context.fiber();
                if let Some(config) = fiber.config::<Configuration>() {
                    dependents
                        .entry(config.instance.name.clone())
                        .or_default()
                        .push(name.clone());
                    break;
                }
                parent = fiber.parent_context();
            }
            let current = instance.data.current.lock().upgrade();
            let component = current.as_ref().map(|g| g.component.clone()).or_else(|| {
                let group = groups.get(&instance.data.group)?;
                let key = (
                    instance.data.plugin.clone(),
                    instance.data.component.clone(),
                );
                group.current.lock().components.get(&key).cloned()
            });
            let Some(component) = component else { continue };
            let view = instance.fiber.context();
            for (interface, _) in &component.inject {
                let binding = match &current {
                    Some(generation) => generation.context.get_named(interface.key.clone()).ok(),
                    None => view.lookup_named(interface.key.clone()),
                }
                .and_then(|value| value.downcast::<Binding>().ok())
                .filter(|binding| binding.interface.minor >= interface.minor);
                match binding {
                    Some(binding) => {
                        if let Some(provider) = &binding.generation {
                            dependents
                                .entry(provider.instance.name.clone())
                                .or_default()
                                .push(name.clone());
                        }
                    }
                    None => waiting.entry(interface.key.clone()).or_default().push((
                        view.realm_named(&interface.key),
                        interface.minor,
                        name.clone(),
                    )),
                }
            }
            if !replaced.contains(name) {
                providers.push((view, component, name.clone()));
            }
        }
        if !waiting.is_empty() {
            let named_realms = self.named_realms();
            let mut declarations = HashMap::new();
            for (view, component, provider) in &providers {
                let plugin = component.plugin_description();
                let interfaces = declarations
                    .entry(std::ptr::from_ref(plugin))
                    .or_insert_with(|| {
                        plugin
                            .components
                            .iter()
                            .flat_map(|item| &item.provides)
                            .map(|name| Interface::parse(name).expect("已校验的接口声明"))
                            .collect::<Vec<_>>()
                    });
                let namespace = views::realm_namespace(view);
                for provided in interfaces {
                    let inherited = view.realm_named(&provided.key);
                    // 动态子组件可以继承当前域，也可以选择同一命名空间中的命名域。
                    if let Some(consumers) = waiting.get(&provided.key) {
                        dependents.entry(provider.clone()).or_default().extend(
                            consumers
                                .iter()
                                .filter(|(realm, minor, _)| {
                                    provided.minor >= *minor
                                        && (*realm == inherited
                                            || named_realms.get(realm) == Some(&namespace))
                                })
                                .map(|(_, _, name)| name.clone()),
                        );
                    }
                }
            }
        }
        let mut selected = HashSet::new();
        let mut pending: Vec<_> = roots.into_iter().collect();
        while let Some(name) = pending.pop() {
            if selected.insert(name.clone())
                && let Some(next) = dependents.remove(&name)
            {
                pending.extend(next);
            }
        }
        let mut selected: Vec<_> = selected.into_iter().collect();
        selected.sort_unstable();
        selected
            .into_iter()
            .filter_map(|name| instances.get(&name).cloned())
            .collect()
    }
}

impl Instance {
    pub(super) fn configuration(&self) -> Result<Arc<Configuration>> {
        self.fiber
            .config()
            .ok_or_else(|| error(4, "实例配置已随纤程释放"))
    }

    /// 立即向纤程提交目标配置，返回的任务等待该实例调和结束。
    pub(super) fn set_configuration(
        self: Arc<Self>,
        config: Arc<Value>,
    ) -> impl Future<Output = Result<()>> {
        let updated = self.fiber.update(Configuration {
            instance: self.data.clone(),
            config,
        });
        async move {
            updated.await.map_err(|problem| {
                self.data
                    .error
                    .lock()
                    .as_ref()
                    .map(|error| error.value.clone())
                    .unwrap_or_else(|| kernel_error(problem))
            })
        }
    }
}
