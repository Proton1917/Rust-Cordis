use super::*;
use futures_util::future::{join_all, poll_fn};
use generation::{Binding, Configuration, Exporter};
use std::{
    path::{Component as PathComponent, Path},
    task::{Poll, Waker},
};

struct Frozen {
    states: HashMap<String, payload::State>,
    held: Vec<(Arc<Generation>, Arc<Exporter>)>,
    failure: Option<WireError>,
}

impl Manager {
    pub async fn attach_group(&self, specification: GroupSpec) -> Result<GroupSnapshot> {
        self.transaction(move |manager| async move {
            manager.running()?;
            validate_name(&specification.name)?;
            if specification.restart.max > 5 {
                return Err(error(2, "进程重启上限为5次"));
            }
            if manager
                .0
                .catalog
                .lock()
                .groups
                .contains_key(&specification.name)
            {
                return Err(error(2, "连接分组已经存在"));
            }
            let version = manager.prepare_process(&specification).await?;
            let name = specification.name.clone();
            manager.install_group(name.clone(), Some(specification), version)?;
            Ok(manager
                .snapshot()
                .groups
                .into_iter()
                .find(|group| group.name == name)
                .expect("installed group"))
        })
        .await
    }

    /// 连接来源可以是进程管道或未来的转发通道，内核适配保持相同协议。
    pub async fn attach_connection(
        &self,
        name: String,
        connection: Connection,
    ) -> Result<GroupSnapshot> {
        self.transaction(move |manager| async move {
            manager.running()?;
            validate_name(&name)?;
            if manager.0.catalog.lock().groups.contains_key(&name) {
                return Err(error(2, "连接分组已经存在"));
            }
            let version = match manager
                .prepare_connection(&name, connection.clone(), None)
                .await
            {
                Ok(version) => version,
                Err(problem) => {
                    connection.terminate(&problem.message);
                    manager.reclaim_connection(connection.id());
                    return Err(problem);
                }
            };
            manager.install_group(name.clone(), None, version)?;
            Ok(manager
                .snapshot()
                .groups
                .into_iter()
                .find(|group| group.name == name)
                .expect("installed group"))
        })
        .await
    }

    fn install_group(
        &self,
        name: String,
        specification: Option<GroupSpec>,
        version: Arc<GroupGeneration>,
    ) -> Result<()> {
        let group = Arc::new(Group {
            name: name.clone(),
            specification: Mutex::new(specification),
            current: Mutex::new(version.clone()),
            definitions: Mutex::new(version.definitions.clone()),
            restarts: AtomicU32::new(0),
            health: Mutex::new(Health {
                state: "running".into(),
                error: None,
            }),
        });
        self.0.catalog.lock().groups.insert(name, group.clone());
        self.monitor(group, version)
    }

    async fn prepare_process(&self, specification: &GroupSpec) -> Result<Arc<GroupGeneration>> {
        let weak = Arc::downgrade(&self.0);
        let log = Arc::new(move |id, line| {
            if let Some(core) = weak.upgrade() {
                Manager(core).stderr(id, line);
            }
        });
        let process = Peer::launch(
            self.id(),
            &specification.command,
            &specification.directory,
            specification.peer.clone(),
            Arc::new(self.clone()),
            log,
        )
        .await?;
        match self
            .prepare_connection(
                &specification.name,
                process.connection(),
                Some(process.clone()),
            )
            .await
        {
            Ok(version) => Ok(version),
            Err(problem) => {
                let closed = process.stop("插件握手或描述符检查失败").await;
                self.reclaim_connection(process.id());
                if let Err(closed) = closed {
                    self.0
                        .context
                        .logger()
                        .error(saved_error(&closed).to_string());
                }
                Err(problem)
            }
        }
    }

    async fn prepare_connection(
        &self,
        group: &str,
        connection: Connection,
        process: Option<Peer>,
    ) -> Result<Arc<GroupGeneration>> {
        let response = self
            .lifecycle_request(
                &connection,
                "hello",
                value(&payload::Hello {
                    protocol: 1,
                    process: connection.id(),
                    group: group.into(),
                })?,
                self.0.options.handshake_timeout,
            )
            .await?;
        let descriptor: payload::Describe = decode(response.value())?;
        if descriptor.protocol != 1 {
            return Err(error(3, "UP/1协议主版本不匹配"));
        }
        let budget = self.retain_metadata(&connection, metadata::descriptor_bytes(&descriptor)?)?;
        let mut plugins = HashSet::new();
        let mut components = HashMap::new();
        let mut definitions = HashMap::new();
        for plugin in &descriptor.plugins {
            validate_name(&plugin.name)?;
            semver::Version::parse(&plugin.version)
                .map_err(|problem| error(2, problem.to_string()))?;
            if !matches!(plugin.format, 1 | 2) {
                return Err(error(3, "插件格式需要为原生或通用进程"));
            }
            if plugin.format == 1 && plugin.library.is_none_or(|library| library == 0) {
                return Err(error(2, "原生插件描述需要正库编号"));
            }
            if !plugins.insert(plugin.name.clone()) {
                return Err(error(2, "插件名称重复"));
            }
            for info in &plugin.components {
                validate_name(&info.name)?;
                if !info.state_schema.is_empty() && info.state_version_min > info.state_version_max
                {
                    return Err(error(2, "继承状态版本范围无效"));
                }
                let key = (plugin.name.clone(), info.name.clone());
                if components.contains_key(&key) {
                    return Err(error(2, "组件名称重复"));
                }
                let inject = info
                    .inject
                    .iter()
                    .map(|dependency| {
                        Ok((
                            Interface::parse(&dependency.interface)?,
                            dependency.required,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let provides = info
                    .provides
                    .iter()
                    .map(|interface| Interface::parse(interface))
                    .collect::<Result<Vec<_>>>()?;
                for event in &info.listens {
                    Interface::parse(event)?;
                }
                let copied = metadata::component_bytes(info)?
                    .checked_add(metadata::strings([group, plugin.name.as_str()])?)
                    .ok_or_else(|| error(8, "组件元数据长度超出地址空间"))?;
                let parsed = metadata::strings(
                    inject
                        .iter()
                        .map(|(interface, _)| interface)
                        .chain(provides.iter())
                        .flat_map(|interface| [interface.full.as_str(), interface.key.as_str()]),
                )?;
                let component = Arc::new(RemoteComponent {
                    group: group.into(),
                    plugin: plugin.name.clone(),
                    library: plugin.library,
                    info: info.clone(),
                    inject,
                    provides,
                    connection: connection.clone(),
                    _metadata: self.retain_metadata(
                        &connection,
                        copied
                            .checked_add(parsed)
                            .ok_or_else(|| error(8, "组件元数据长度超出地址空间"))?,
                    )?,
                });
                definitions.insert(
                    key.clone(),
                    generation::definition(Arc::downgrade(&self.0), component.clone()),
                );
                components.insert(key, component);
            }
        }
        Ok(Arc::new(GroupGeneration {
            connection,
            process,
            descriptor,
            components,
            definitions,
            _metadata: budget,
        }))
    }

    async fn resolve(&self, component: &RemoteComponent, config: &Value) -> Result<()> {
        payload::persistent_value(config).map_err(|problem| error(2, problem.to_string()))?;
        let response = self
            .lifecycle_request(
                &component.connection,
                "resolve",
                value(&payload::Resolve {
                    plugin: component.plugin.clone(),
                    component: component.info.name.clone(),
                    library: component.library,
                    config: config.clone(),
                })?,
                self.0.options.apply_timeout,
            )
            .await?;
        decode(response.value())
    }

    pub async fn mount(
        &self,
        specification: InstanceSpec,
        inherited: Option<payload::State>,
    ) -> Result<Fiber> {
        self.mount_in(self.0.context.clone(), specification, inherited)
            .await
    }

    pub async fn mount_in(
        &self,
        view: Context,
        specification: InstanceSpec,
        inherited: Option<payload::State>,
    ) -> Result<Fiber> {
        self.transaction(move |manager| async move {
            manager.running()?;
            manager.check_view(&view)?;
            validate_name(&specification.name)?;
            if manager
                .0
                .catalog
                .lock()
                .instances
                .contains_key(&specification.name)
            {
                return Err(error(2, "实例名称已经存在"));
            }
            if let Some(state) = &inherited {
                payload::persistent_value(&state.data)
                    .map_err(|problem| error(2, problem.to_string()))?;
            }
            let group = manager.group(&specification.group)?;
            let current = group.current.lock().clone();
            let key = (
                specification.plugin.clone(),
                specification.component.clone(),
            );
            let component = current
                .components
                .get(&key)
                .ok_or_else(|| error(2, "描述符中缺少目标组件"))?;
            manager.resolve(component, &specification.config).await?;
            let definition = group
                .definitions
                .lock()
                .get(&key)
                .cloned()
                .ok_or_else(|| error(4, "组件定义已退役"))?;
            let data = Arc::new(InstanceData {
                name: specification.name.clone(),
                group: specification.group,
                plugin: specification.plugin,
                component: specification.component,
                current: Mutex::new(Weak::new()),
                config: Mutex::new(specification.config.clone()),
                inherited: Mutex::new(inherited.clone()),
                error: Mutex::new(None),
            });
            let fiber = view
                .mount_with(
                    &definition,
                    Configuration {
                        instance: data.clone(),
                        config: specification.config,
                    },
                )
                .map_err(kernel_error)?;
            let mut catalog = manager.0.catalog.lock();
            if let Some(state) = inherited {
                catalog
                    .checkpoints
                    .insert(specification.name.clone(), state);
            }
            catalog.instances.insert(
                specification.name,
                Arc::new(Instance {
                    data,
                    fiber: fiber.clone(),
                }),
            );
            Ok(fiber)
        })
        .await
    }

    pub async fn update(&self, name: &str, config: Value) -> Result<()> {
        let name = name.to_owned();
        self.transaction(move |manager| async move {
            manager.running()?;
            let instance = manager.instance(&name)?;
            let group = manager.group(&instance.data.group)?;
            let current = group.current.lock().clone();
            let key = (
                instance.data.plugin.clone(),
                instance.data.component.clone(),
            );
            let component = current
                .components
                .get(&key)
                .ok_or_else(|| error(2, "描述符中缺少目标组件"))?;
            manager.resolve(component, &config).await?;
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
        })
        .await
    }

    pub async fn dispose(&self, name: &str) -> Result<()> {
        let name = name.to_owned();
        self.transaction(move |manager| async move {
            let instance = manager.instance(&name)?;
            let result = instance.fiber.dispose().await.map_err(kernel_error);
            let removed = {
                let mut catalog = manager.0.catalog.lock();
                catalog.checkpoints.remove(&name);
                catalog.instances.remove(&name)
            };
            drop(removed);
            result
        })
        .await
    }

    pub fn saved_states(&self) -> HashMap<String, payload::State> {
        self.0.catalog.lock().checkpoints.clone()
    }

    pub async fn checkpoint(&self) -> Result<HashMap<String, payload::State>> {
        self.transaction(|manager| async move {
            let names = manager
                .0
                .catalog
                .lock()
                .instances
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let frozen = manager.export(&names, false).await;
            manager
                .0
                .catalog
                .lock()
                .checkpoints
                .extend(frozen.states.clone());
            let thawed = manager.thaw(&frozen).await;
            if let Some(problem) = &frozen.failure {
                return Err(problem.clone());
            }
            thawed?;
            Ok(frozen.states)
        })
        .await
    }

    async fn export(&self, names: &[String], stop_on_error: bool) -> Frozen {
        let instances = {
            let catalog = self.0.catalog.lock();
            names
                .iter()
                .filter_map(|name| catalog.instances.get(name).cloned())
                .collect::<Vec<_>>()
        };
        let mut frozen = Frozen {
            states: HashMap::new(),
            held: Vec::new(),
            failure: None,
        };
        for instance in instances {
            let generation = instance.data.current.lock().upgrade();
            let Some(generation) = generation else {
                continue;
            };
            if !generation.is_current() || instance.fiber.state() != cordis::FiberState::Active {
                continue;
            }
            let exporter = generation.exporter.lock().clone();
            let Some(exporter) = exporter else {
                continue;
            };
            frozen.held.push((generation.clone(), exporter.clone()));
            let result = async {
                let response = self
                    .lifecycle_request(
                        exporter.object.peer(),
                        "export",
                        value(&payload::Export {
                            object: exporter.object.id(),
                            context: exporter.context,
                        })?,
                        self.0.options.export_timeout,
                    )
                    .await?;
                let result: payload::ValueResult = decode(response.value())?;
                payload::persistent_value(&result.value)
                    .map_err(|problem| error(2, problem.to_string()))?;
                Ok(payload::State {
                    schema: exporter.schema.clone(),
                    version: exporter.version,
                    data: result.value,
                })
            }
            .await;
            match result {
                Ok(state) => {
                    frozen.states.insert(instance.data.name.clone(), state);
                }
                Err(problem) => {
                    self.0
                        .context
                        .logger()
                        .error(saved_error(&problem).to_string());
                    frozen.failure.get_or_insert(problem);
                    if stop_on_error {
                        break;
                    }
                }
            }
        }
        frozen
    }

    async fn thaw(&self, frozen: &Frozen) -> Result<()> {
        let mut failure = None;
        for (_, exporter) in &frozen.held {
            if !exporter.object.peer().is_alive() {
                continue;
            }
            let result = async {
                let response = self
                    .lifecycle_request(
                        exporter.object.peer(),
                        "thaw",
                        value(&payload::Thaw {
                            object: exporter.object.id(),
                            context: exporter.context,
                        })?,
                        self.0.options.cleanup_timeout,
                    )
                    .await?;
                decode::<()>(response.value())
            }
            .await;
            if let Err(problem) = result {
                exporter.object.peer().terminate("状态解冻失败");
                failure.get_or_insert(problem);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub async fn replace_group(&self, specification: GroupSpec) -> Result<ReplaceReport> {
        self.transaction(move |manager| async move { manager.replace(specification).await })
            .await
    }

    async fn replace(&self, specification: GroupSpec) -> Result<ReplaceReport> {
        self.running()?;
        if specification.restart.max > 5 {
            return Err(error(2, "进程重启上限为5次"));
        }
        let group = self.group(&specification.name)?;
        let previous = group.current.lock().clone();
        let candidate = self.prepare_process(&specification).await?;
        if let Err(problem) = self.preflight(&group, &candidate).await {
            self.retire(&candidate).await?;
            return Err(problem);
        }
        let instances = self.group_instances(&group.name);
        let names: Vec<_> = instances
            .iter()
            .map(|instance| instance.data.name.clone())
            .collect();
        let frozen = self.export(&names, true).await;
        if let Some(problem) = &frozen.failure {
            let problem = problem.clone();
            let thawed = self.thaw(&frozen).await;
            self.retire(&candidate).await?;
            thawed?;
            return Err(problem);
        }
        self.0
            .catalog
            .lock()
            .checkpoints
            .extend(frozen.states.clone());
        let inherited: HashMap<_, _> = instances
            .iter()
            .map(|instance| {
                (
                    instance.data.name.clone(),
                    frozen
                        .states
                        .get(&instance.data.name)
                        .cloned()
                        .or_else(|| instance.data.inherited.lock().clone()),
                )
            })
            .collect();
        let before = self.epochs();
        group.health.lock().state = "replacing".into();
        self.inherit(&inherited);
        self.activate(&group, candidate.clone())?;
        let mut report = ReplaceReport {
            group: group.name.clone(),
            previous_connection: previous.connection.id(),
            candidate_connection: candidate.connection.id(),
            affected_instances: 0,
            succeeded: false,
            rolled_back: false,
            failure: None,
        };
        let changed = self.stabilize(&before, &names).await;
        let accepted = match changed {
            Ok(changed) => {
                report.affected_instances = changed.len();
                self.confirm(&changed, &candidate.connection).await
            }
            Err(problem) => Err(problem),
        };
        if let Err(problem) = accepted {
            self.inherit(&inherited);
            self.activate(&group, previous.clone())?;
            let rollback = self.stabilize(&before, &names).await;
            report.rolled_back = rollback.is_ok();
            report.failure = Some(Failure::from(&problem));
            group.health.lock().state = if rollback.is_ok() {
                "running"
            } else {
                "failed"
            }
            .into();
            group.health.lock().error = Some(saved_error(&problem));
            self.0
                .catalog
                .lock()
                .reports
                .insert(group.name.clone(), report);
            drop(frozen);
            self.retire(&candidate).await?;
            if let Err(rollback) = rollback {
                return Err(error(
                    1,
                    format!(
                        "替换失败：{}；回滚失败：{}",
                        metadata::clipped(&problem.message),
                        metadata::clipped(&rollback.message)
                    ),
                ));
            }
            return Err(problem);
        }
        *group.specification.lock() = Some(specification);
        *group.health.lock() = Health {
            state: "running".into(),
            error: None,
        };
        group.restarts.store(0, Ordering::Release);
        report.succeeded = true;
        self.0
            .catalog
            .lock()
            .reports
            .insert(group.name.clone(), report.clone());
        drop(frozen);
        self.retire(&previous).await?;
        self.monitor(group, candidate)?;
        Ok(report)
    }

    async fn preflight(&self, group: &Group, candidate: &GroupGeneration) -> Result<()> {
        for instance in self.group_instances(&group.name) {
            let key = (
                instance.data.plugin.clone(),
                instance.data.component.clone(),
            );
            let component = candidate
                .components
                .get(&key)
                .ok_or_else(|| error(3, "候选进程缺少正在使用的组件"))?;
            let config = instance.data.config.lock().clone();
            self.resolve(component, &config).await?;
        }
        let instances = self
            .0
            .catalog
            .lock()
            .instances
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut needed = HashMap::<String, u32>::new();
        for instance in instances {
            let current = instance.data.current.lock().upgrade();
            let Some(current) = current else {
                continue;
            };
            for (interface, _) in &current.component.inject {
                let Ok(value) = current.context.get_named(interface.key.clone()) else {
                    continue;
                };
                if let Some(binding) = value.downcast_ref::<Binding>()
                    && binding
                        .generation
                        .as_ref()
                        .is_some_and(|source| source.instance.group == group.name)
                {
                    needed
                        .entry(binding.interface.key.clone())
                        .and_modify(|minor| *minor = (*minor).max(binding.interface.minor))
                        .or_insert(binding.interface.minor);
                }
            }
        }
        let mut provided = HashMap::<String, u32>::new();
        for component in candidate.components.values() {
            for interface in &component.provides {
                provided
                    .entry(interface.key.clone())
                    .and_modify(|minor| *minor = (*minor).max(interface.minor))
                    .or_insert(interface.minor);
            }
        }
        if needed
            .iter()
            .any(|(key, minor)| provided.get(key).is_none_or(|candidate| candidate < minor))
        {
            return Err(error(3, "候选进程的提供接口与当前使用方不兼容"));
        }
        Ok(())
    }

    fn activate(&self, group: &Group, candidate: Arc<GroupGeneration>) -> Result<()> {
        let previous = std::mem::replace(
            &mut *group.definitions.lock(),
            candidate.definitions.clone(),
        );
        *group.current.lock() = candidate.clone();
        for (key, old) in previous {
            match candidate.definitions.get(&key) {
                Some(next) => {
                    self.0.context.registry().replace(&old, next);
                }
                None => self
                    .0
                    .context
                    .registry()
                    .forget(&old)
                    .map_err(kernel_error)?,
            }
        }
        Ok(())
    }

    fn epochs(&self) -> HashMap<String, cordis::Epoch> {
        self.0
            .catalog
            .lock()
            .instances
            .iter()
            .map(|(name, instance)| (name.clone(), instance.fiber.epoch()))
            .collect()
    }

    fn group_instances(&self, group: &str) -> Vec<Arc<Instance>> {
        self.0
            .catalog
            .lock()
            .instances
            .values()
            .filter(|instance| instance.data.group == group)
            .cloned()
            .collect()
    }

    fn inherit(&self, states: &HashMap<String, Option<payload::State>>) {
        let instances = {
            let catalog = self.0.catalog.lock();
            states
                .iter()
                .filter_map(|(name, state)| {
                    catalog
                        .instances
                        .get(name)
                        .map(|instance| (instance.clone(), state.clone()))
                })
                .collect::<Vec<_>>()
        };
        for (instance, state) in instances {
            *instance.data.inherited.lock() = state;
        }
    }

    async fn stabilize(
        &self,
        before: &HashMap<String, cordis::Epoch>,
        explicit: &[String],
    ) -> Result<Vec<Fiber>> {
        let future = async {
            loop {
                let instances = self
                    .0
                    .catalog
                    .lock()
                    .instances
                    .values()
                    .cloned()
                    .collect::<Vec<_>>();
                let changed: Vec<_> = instances
                    .into_iter()
                    .filter(|instance| {
                        explicit.contains(&instance.data.name)
                            || before
                                .get(&instance.data.name)
                                .is_some_and(|epoch| *epoch != instance.fiber.epoch())
                    })
                    .collect();
                for (instance, result) in changed
                    .iter()
                    .zip(join_all(changed.iter().map(|instance| instance.fiber.settled())).await)
                {
                    result.map_err(|problem| {
                        instance
                            .data
                            .error
                            .lock()
                            .as_ref()
                            .map(|problem| problem.value.clone())
                            .unwrap_or_else(|| kernel_error(problem))
                    })?;
                }
                if changed.iter().any(|instance| !instance.fiber.is_settled()) {
                    continue;
                }
                if changed
                    .iter()
                    .any(|instance| instance.fiber.state() != cordis::FiberState::Active)
                {
                    return Err(error(4, "受影响实例未进入Active"));
                }
                return Ok(changed
                    .into_iter()
                    .map(|instance| instance.fiber.clone())
                    .collect());
            }
        };
        tokio::time::timeout(self.0.options.swap_timeout, future)
            .await
            .map_err(|_| error(6, "实例稳定等待超时"))?
    }

    async fn confirm(&self, fibers: &[Fiber], connection: &Connection) -> Result<()> {
        if fibers.is_empty() || self.0.options.confirm.is_zero() {
            return Ok(());
        }
        struct Changed {
            failed: bool,
            waker: Option<Waker>,
        }
        let changed = Arc::new(Mutex::new(Changed {
            failed: false,
            waker: None,
        }));
        let observed = changed.clone();
        let fibers = fibers.to_vec();
        let listener = self
            .0
            .context
            .on::<cordis::InternalStatus>(move |(fiber, _)| {
                if fibers.contains(fiber)
                    && fiber.state() != cordis::FiberState::Active
                    && (fiber.state() == cordis::FiberState::Failed
                        || !fiber.is_alive()
                        || fiber.epoch() != cordis::Epoch::Inactive)
                {
                    let waker = {
                        let mut changed = observed.lock();
                        changed.failed = true;
                        changed.waker.take()
                    };
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                }
                None
            })
            .map_err(kernel_error)?;
        let failed = poll_fn(|context| {
            let mut changed = changed.lock();
            if changed.failed {
                Poll::Ready(())
            } else {
                changed.waker = Some(context.waker().clone());
                Poll::Pending
            }
        });
        let result = tokio::select! {
            _ = tokio::time::sleep(self.0.options.confirm) => Ok(()),
            _ = failed => Err(error(4, "确认期内实例离开Active")),
            reason = connection.closed() => Err(error(4, reason)),
        };
        listener.dispose().await.map_err(kernel_error)?;
        result
    }

    fn monitor(&self, group: Arc<Group>, version: Arc<GroupGeneration>) -> Result<()> {
        let weak = Arc::downgrade(&self.0);
        let mut stopped = self.0.stop.subscribe();
        self.0
            .context
            .effect_async(
                format!("connection-monitor:{}", version.connection.id()),
                move |_| async move {
                    tokio::select! {
                        reason = version.connection.closed() => {
                            if let Some(core) = weak.upgrade() {
                                let manager = Manager(core);
                                manager.reclaim_connection(version.connection.id());
                                manager.queue_recovery(group, version, reason).await;
                            }
                        }
                        _ = stopped.changed() => {}
                    }
                    Ok(())
                },
            )
            .map_err(kernel_error)?;
        Ok(())
    }

    async fn queue_recovery(
        &self,
        group: Arc<Group>,
        version: Arc<GroupGeneration>,
        reason: String,
    ) {
        let sender = {
            let queue = self.0.commands.lock();
            if !queue.accepting {
                return;
            }
            queue.sender.clone()
        };
        let (reply, _result) = oneshot::channel();
        let work: Command = Box::new(Action {
            callback: move |manager: Manager| async move {
                manager.recover(group, version, reason).await
            },
            reply,
        });
        if sender.send(work).await.is_err() && !self.0.stopping.load(Ordering::Acquire) {
            self.0.context.logger().error("进程回收操作队列已结束");
        }
    }

    async fn recover(
        &self,
        group: Arc<Group>,
        version: Arc<GroupGeneration>,
        reason: String,
    ) -> Result<()> {
        if group.current.lock().connection.id() != version.connection.id()
            || self.0.stopping.load(Ordering::Acquire)
        {
            return Ok(());
        }
        let problem = error(4, metadata::clipped(&reason));
        self.withdraw(&group, &version, problem).await;
        let specification = group.specification.lock().clone();
        let Some(specification) = specification else {
            return Ok(());
        };
        let mut shutdown = self.0.stop.subscribe();
        while group.restarts.load(Ordering::Acquire) < specification.restart.max {
            if !self.0.context.is_current() || *shutdown.borrow() {
                return Ok(());
            }
            let attempt = group.restarts.load(Ordering::Acquire);
            group.health.lock().state = "backoff".into();
            tokio::select! {
                _ = tokio::time::sleep(specification.restart.backoff.saturating_mul(1u32 << attempt)) => {},
                _ = shutdown.changed() => return Ok(()),
            }
            if *shutdown.borrow() {
                return Ok(());
            }
            group.restarts.fetch_add(1, Ordering::AcqRel);
            group.health.lock().state = "restarting".into();
            let candidate = match self.prepare_process(&specification).await {
                Ok(candidate) => candidate,
                Err(problem) => {
                    group.health.lock().error = Some(saved_error(&problem));
                    continue;
                }
            };
            if let Err(problem) = self.preflight(&group, &candidate).await {
                group.health.lock().error = Some(saved_error(&problem));
                self.retire(&candidate).await?;
                continue;
            }
            let instances = self.group_instances(&group.name);
            let inherited = {
                let catalog = self.0.catalog.lock();
                instances
                    .iter()
                    .map(|instance| {
                        (
                            instance.data.name.clone(),
                            catalog.checkpoints.get(&instance.data.name).cloned(),
                        )
                    })
                    .collect()
            };
            self.inherit(&inherited);
            self.activate(&group, candidate.clone())?;
            let outcomes =
                join_all(instances.iter().map(|instance| instance.fiber.settled())).await;
            if !candidate.connection.is_alive() {
                self.reclaim_connection(candidate.connection.id());
                self.withdraw(
                    &group,
                    &candidate,
                    error(4, candidate.connection.closed().await),
                )
                .await;
                continue;
            }
            if let Some(problem) = outcomes.into_iter().find_map(std::result::Result::err) {
                group.health.lock().error = Some(saved_error(&kernel_error(problem)));
            }
            group.health.lock().state = "running".into();
            self.monitor(group, candidate)?;
            return Ok(());
        }
        group.health.lock().state = "failed".into();
        Ok(())
    }

    async fn withdraw(&self, group: &Group, version: &GroupGeneration, problem: WireError) {
        *group.health.lock() = Health {
            state: "failed".into(),
            error: Some(problem.clone()),
        };
        let previous = group.definitions.lock().clone();
        let mut failed = HashMap::new();
        for (key, definition) in previous {
            let failure = problem.clone();
            let next = ComponentDef::new(
                format!("failed:{}:{}", version.connection.id(), failed.len()),
                Inject::new(),
                move |_, config: Configuration| {
                    let failure = failure.clone();
                    async move {
                        *config.instance.error.lock() = Some(metadata::RetainedError {
                            value: failure.clone(),
                            _budget: None,
                        });
                        Err(kernel(failure))
                    }
                },
            );
            self.0.context.registry().replace(&definition, &next);
            failed.insert(key, next);
        }
        *group.definitions.lock() = failed;
        for result in join_all(
            self.group_instances(&group.name)
                .iter()
                .map(|instance| instance.fiber.settled()),
        )
        .await
        {
            if let Err(problem) = result {
                self.0
                    .context
                    .logger()
                    .debug(metadata::clipped(&problem.to_string()));
            }
        }
    }

    fn reclaim_connection(&self, connection: u64) {
        self.reclaim_routes(connection);
        {
            let mut effects = self.0.effects.lock();
            effects.records.retain(|(owner, _), _| *owner != connection);
            effects.next.remove(&connection);
        }
        self.0.metadata.lock().remove(&connection);
    }

    async fn retire(&self, version: &GroupGeneration) -> Result<()> {
        let result = match &version.process {
            Some(process) if process.is_alive() => {
                process.shutdown(self.0.options.cleanup_timeout).await
            }
            Some(process) => {
                process.wait_exit().await?;
                Ok(())
            }
            None => {
                version.connection.terminate("连接分组已退役");
                Ok(())
            }
        };
        self.reclaim_connection(version.connection.id());
        result
    }

    pub(super) async fn shutdown_contents(&self) -> Result<()> {
        let sender = {
            let mut queue = self.0.commands.lock();
            queue.accepting = false;
            queue.sender.clone()
        };
        self.0.stop.send_replace(true);
        let mut finished = self.0.finished.subscribe();
        loop {
            if let Some(result) = finished.borrow_and_update().clone() {
                return result;
            }
            if sender.is_closed() {
                if let Some(result) = finished.borrow_and_update().clone() {
                    return result;
                }
                let mut failure = error(4, "核心管理操作任务已结束");
                if let Err(cleanup) = self.close_all().await {
                    failure.message = format!(
                        "{}；清理失败(code {})：{}",
                        failure.message,
                        cleanup.code,
                        metadata::clipped(&cleanup.message)
                    );
                    failure.detail = Value::Map(vec![
                        (
                            Value::Text("cleanup_code".into()),
                            Value::Integer(cleanup.code.into()),
                        ),
                        (
                            Value::Text("cleanup_message".into()),
                            Value::Text(cleanup.message),
                        ),
                        (Value::Text("cleanup_detail".into()), cleanup.detail),
                    ]);
                }
                self.0.finished.send_replace(Some(Err(failure)));
                continue;
            }
            tokio::select! {
                result = finished.changed() => result.map_err(|_| error(4, "核心管理器结束状态已关闭"))?,
                _ = sender.closed() => {},
            }
        }
    }

    pub(super) async fn close_all(&self) -> Result<()> {
        self.0.stopping.store(true, Ordering::Release);
        self.0.stop.send_replace(true);
        let names = self
            .0
            .catalog
            .lock()
            .instances
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let frozen = self.export(&names, false).await;
        self.0
            .catalog
            .lock()
            .checkpoints
            .extend(frozen.states.clone());
        let (instances, groups) = {
            let catalog = self.0.catalog.lock();
            (
                catalog.instances.values().cloned().collect::<Vec<_>>(),
                catalog.groups.values().cloned().collect::<Vec<_>>(),
            )
        };
        let mut failure = frozen.failure.clone();
        for result in join_all(instances.iter().map(|instance| instance.fiber.dispose())).await {
            if let Err(problem) = result {
                failure.get_or_insert_with(|| kernel_error(problem));
            }
        }
        drop(frozen);
        for group in groups {
            let definitions = std::mem::take(&mut *group.definitions.lock());
            for definition in definitions.values() {
                if let Err(problem) = self.0.context.registry().forget(definition) {
                    failure.get_or_insert_with(|| kernel_error(problem));
                }
            }
            let current = group.current.lock().clone();
            if let Err(problem) = self.retire(&current).await {
                failure.get_or_insert(problem);
            }
            group.health.lock().state = "stopped".into();
        }
        let discarded = std::mem::take(&mut self.0.catalog.lock().instances);
        drop(discarded);
        failure.map_or(Ok(()), Err)
    }
}

fn validate_name(name: &str) -> Result<()> {
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(PathComponent::Normal(part)), None) if part == name => Ok(()),
        _ => Err(error(2, "名称需要为一个完整名称段")),
    }
}
