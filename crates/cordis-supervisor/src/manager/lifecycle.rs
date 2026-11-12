//! DOC-11 SW-1–SW-9、DOC-15 CP-4/CP-6/CP-7：核心持有分组与实例的生命周期决策。
//! 事务在有界工作队列中执行，等待者取消后仍完成提交或回滚；失败清理覆盖候选资源。
use super::*;
use futures_util::future::{join_all, poll_fn};
use generation::{Binding, Configuration, Exporter};
use std::{
    path::{Component as PathComponent, Path},
    task::{Poll, Waker},
};

/// SW-5：原状态与导出器持有关系；提交或解冻完成后统一释放。
pub(super) struct Frozen {
    pub(super) states: HashMap<String, payload::State>,
    pub(super) held: Vec<(Arc<Generation>, Arc<Exporter>)>,
    pub(super) failure: Option<WireError>,
}

/// SW-5–SW-8：一次替换的原快照、受影响实例名与变更前纪元。
pub(super) struct Transition {
    pub frozen: Frozen,
    pub inherited: HashMap<String, Option<payload::State>>,
    pub names: Vec<String>,
    pub before: HashMap<String, cordis::Epoch>,
}

impl Manager {
    /// SW-5：取得可逆冻结与原快照；任一导出失败时解冻已经访问的导出器。
    pub(super) async fn prepare_transition(
        &self,
        instances: &[Arc<Instance>],
    ) -> Result<Transition> {
        let names: Vec<_> = instances
            .iter()
            .map(|instance| instance.data.name.clone())
            .collect();
        let frozen = self.export(&names, true).await;
        if let Some(problem) = &frozen.failure {
            let problem = problem.clone();
            self.thaw(&frozen).await?;
            return Err(problem);
        }
        self.0
            .catalog
            .lock()
            .checkpoints
            .extend(frozen.states.clone());
        let inherited = instances
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
        Ok(Transition {
            frozen,
            inherited,
            names,
            before: self.epochs(),
        })
    }
    /// CP-1、CP-6：准备独立进程并握手，接受其描述符后建立核心分组。
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

    /// CP-4、CP-7：启动拥有型进程；握手失败时先终止并回收，再交回原错误。
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
        self.generation_from_description(group, connection, process, descriptor)
    }

    /// SW-3、CP-2/CP-8：把经过受限解码的描述符变为核心定义，校验名称和接口并登记元数据额度。
    /// 构造失败时局部拥有者释放已取得的额度，调用方负责退役候选连接。
    pub(super) fn generation_from_description(
        &self,
        group: &str,
        connection: Connection,
        process: Option<Peer>,
        descriptor: payload::Describe,
    ) -> Result<Arc<GroupGeneration>> {
        if descriptor.protocol != 1 {
            return Err(error(3, "UP/1协议主版本不匹配"));
        }
        let budget = self.retain_metadata(&connection, metadata::descriptor_bytes(&descriptor)?)?;
        let descriptor = Arc::new(descriptor);
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
                let component = self.component_from_description(
                    group,
                    &connection,
                    plugin,
                    info,
                    &descriptor,
                    &budget,
                )?;
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

    /// SW-3、ADR-0016：解析组件依赖与提供声明，并将原描述符和字节额度绑定到组件寿命。
    pub(super) fn component_from_description(
        &self,
        group: &str,
        connection: &Connection,
        plugin: &payload::Plugin,
        info: &payload::Component,
        descriptor: &Arc<payload::Describe>,
        budget: &Arc<MetadataLease>,
    ) -> Result<Arc<RemoteComponent>> {
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
        let retained = self.retain_metadata(
            connection,
            copied
                .checked_add(parsed)
                .ok_or_else(|| error(8, "组件元数据长度超出地址空间"))?,
        )?;
        Ok(Arc::new(RemoteComponent {
            group: group.into(),
            plugin: plugin.name.clone(),
            library: plugin.library,
            info: info.clone(),
            inject,
            provides,
            connection: connection.clone(),
            source: descriptor.clone(),
            _source_metadata: budget.clone(),
            _metadata: retained,
        }))
    }

    /// SW-3/SW-4：远端配置检查先于核心提交，原生请求携带准确库编号。
    pub(super) async fn resolve(&self, component: &RemoteComponent, config: &Value) -> Result<()> {
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

    /// DOC-15 §6.4、CP-4：在指定核心视图挂载实例；先完成远端配置检查，再登记纤程与继承状态。
    /// 此操作由事务队列持有，等待句柄取消仅结束等待。
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
            manager.install_instance(view, specification, definition, inherited)
        })
        .await
    }

    /// DOC-11 §5.2、T-LIF-1：已验证的配置进入内核，目录记录和检查点归属同一个实例名。
    fn install_instance(
        &self,
        view: Context,
        specification: InstanceSpec,
        definition: ComponentDef,
        inherited: Option<payload::State>,
    ) -> Result<Fiber> {
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
        let mut catalog = self.0.catalog.lock();
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
    }

    /// S-FBR-4、CP-4：配置通过远端检查后进入Fiber.update，等待实际迁移结果。
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

    /// T-EFF-2、CP-6：从目录移出显式实例，并等待内核按依赖次序清理。
    pub async fn dispose(&self, name: &str) -> Result<()> {
        let name = name.to_owned();
        self.transaction(move |manager| async move {
            manager.running()?;
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

    /// DOC-11 §7：读取最近成功替换在 SW-5 导出的原快照；缺席导出器的实例记录 None。
    pub fn last_replacement_states(&self, group: &str) -> HashMap<String, Option<payload::State>> {
        self.0
            .catalog
            .lock()
            .replacement_states
            .get(group)
            .cloned()
            .unwrap_or_default()
    }

    /// DOC-11 SW-9：成功确认时保存本次导出结果，供运行时与上一版本引用共同持久化。
    pub(super) fn retain_replacement_states(&self, group: &str, names: &[String], frozen: &Frozen) {
        let states = names
            .iter()
            .map(|name| (name.clone(), frozen.states.get(name).cloned()))
            .collect();
        self.0
            .catalog
            .lock()
            .replacement_states
            .insert(group.into(), states);
    }

    /// CP-7：先释放该组实例与定义，再退役进程；独立清理失败分别保留。
    pub async fn detach_group(&self, name: &str) -> Result<()> {
        let name = name.to_owned();
        self.transaction(move |manager| async move {
            manager.running()?;
            let group = manager.group(&name)?;
            let instances = manager.group_instances(&name);
            manager.0.catalog.lock().groups.remove(&name);
            let mut failure = None;
            for result in join_all(instances.iter().map(|instance| instance.fiber.dispose())).await
            {
                if let Err(problem) = result {
                    failure.get_or_insert_with(|| kernel_error(problem));
                }
            }
            let definitions = std::mem::take(&mut *group.definitions.lock());
            for definition in definitions.values() {
                if let Err(problem) = manager.0.context.registry().forget(definition) {
                    failure.get_or_insert_with(|| kernel_error(problem));
                }
            }
            let current = group.current.lock().clone();
            if let Err(problem) = manager.retire(&current).await {
                failure.get_or_insert(problem);
            }
            let discarded = {
                let mut catalog = manager.0.catalog.lock();
                instances
                    .iter()
                    .filter_map(|instance| {
                        catalog.checkpoints.remove(&instance.data.name);
                        catalog.instances.remove(&instance.data.name)
                    })
                    .collect::<Vec<_>>()
            };
            drop(discarded);
            failure.map_or(Ok(()), Err)
        })
        .await
    }

    pub async fn checkpoint_instances(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, payload::State>> {
        let names = names.to_vec();
        self.transaction(move |manager| async move {
            manager.running()?;
            for name in &names {
                manager.instance(name)?;
            }
            manager.checkpoint_selected(&names).await
        })
        .await
    }

    /// DOC-11 §9、CP-11：停止结构变更后导出最终状态；失败解冻，成功保持至shutdown。
    pub async fn export_shutdown(&self) -> Result<HashMap<String, payload::State>> {
        self.transaction(|manager| async move {
            if let Some(frozen) = manager.0.shutdown_export.lock().as_ref() {
                return Ok(frozen.states.clone());
            }
            manager.running()?;
            manager.0.exported.store(true, Ordering::Release);
            let names = manager
                .0
                .catalog
                .lock()
                .instances
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let frozen = manager.export(&names, false).await;
            if let Some(problem) = &frozen.failure {
                let problem = problem.clone();
                let thawed = manager.thaw(&frozen).await;
                manager.0.exported.store(false, Ordering::Release);
                thawed?;
                return Err(problem);
            }
            let states = frozen.states.clone();
            manager.0.catalog.lock().checkpoints.extend(states.clone());
            *manager.0.shutdown_export.lock() = Some(frozen);
            Ok(states)
        })
        .await
    }

    pub async fn checkpoint(&self) -> Result<HashMap<String, payload::State>> {
        self.transaction(|manager| async move {
            manager.running()?;
            let names = manager
                .0
                .catalog
                .lock()
                .instances
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            manager.checkpoint_selected(&names).await
        })
        .await
    }

    async fn checkpoint_selected(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, payload::State>> {
        let frozen = self.export(names, false).await;
        self.0
            .catalog
            .lock()
            .checkpoints
            .extend(frozen.states.clone());
        let thawed = self.thaw(&frozen).await;
        if let Some(problem) = &frozen.failure {
            return Err(problem.clone());
        }
        thawed?;
        Ok(frozen.states)
    }

    /// DOC-11 §5、SW-5：对选定活跃实例执行有截止时间的导出，并保留源侧持有关系。
    pub(super) async fn export(&self, names: &[String], stop_on_error: bool) -> Frozen {
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
            match self.export_state(&exporter).await {
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

    /// SW-5、CP-4：调用活跃导出器并验证持久状态值，截止时间或编码错误交回冻结事务。
    async fn export_state(&self, exporter: &Exporter) -> Result<payload::State> {
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

    /// SW-5、DOC-11 §7：解除已冻结写入；连接失效由其故障回收路径处理。
    pub(super) async fn thaw(&self, frozen: &Frozen) -> Result<()> {
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

    /// DOC-11 SW-1–SW-9：从当前导出状态替换分组，失败恢复原版本及原快照。
    pub async fn replace_group(&self, specification: GroupSpec) -> Result<ReplaceReport> {
        self.restore_group(specification, HashMap::new()).await
    }

    /// DOC-11 §7：以保存的原快照恢复上一分组版本。
    ///
    /// states 按实例名提供继承覆盖；本次操作失败时使用当前版本在 SW-5 导出的状态恢复。
    pub async fn restore_group(
        &self,
        specification: GroupSpec,
        states: HashMap<String, Option<payload::State>>,
    ) -> Result<ReplaceReport> {
        let group = specification.name.clone();
        let committed = Arc::new(AtomicBool::new(false));
        let marker = committed.clone();
        self.transaction(move |manager| async move {
            manager.validate_restore_states(&specification.name, None, &states)?;
            manager
                .replace_with_states(specification, states, Some(marker))
                .await
        })
        .await
        .map_err(|problem| {
            replacement_failure(&group, None, committed.load(Ordering::Acquire), problem)
        })
    }

    /// DOC-15 §8.7：分组换代和旧映像阈值重启共用当前状态迁移。
    pub(super) async fn replace(&self, specification: GroupSpec) -> Result<ReplaceReport> {
        self.replace_with_states(specification, HashMap::new(), None)
            .await
    }

    /// SW-3–SW-9、ADR-0013：候选预检后冻结旧状态，重建并确认；失败恢复原连接与原快照。
    /// states为显式手工回滚状态，自动失败回滚始终使用本次替换前导出值。
    async fn replace_with_states(
        &self,
        specification: GroupSpec,
        states: HashMap<String, Option<payload::State>>,
        committed: Option<Arc<AtomicBool>>,
    ) -> Result<ReplaceReport> {
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
        let transition = match self.prepare_transition(&instances).await {
            Ok(transition) => transition,
            Err(problem) => {
                self.retire(&candidate).await?;
                return Err(problem);
            }
        };
        group.health.lock().state = "replacing".into();
        let mut candidate_states = transition.inherited.clone();
        candidate_states.extend(states);
        self.inherit(&candidate_states);
        self.activate(&group, candidate.clone(), None)?;
        let mut report = ReplaceReport {
            group: group.name.clone(),
            previous_connection: previous.connection.id(),
            candidate_connection: candidate.connection.id(),
            affected_instances: 0,
            committed: false,
            succeeded: false,
            rolled_back: false,
            failure: None,
        };
        if let Err(problem) = self
            .accept_group(&candidate, &transition, &mut report)
            .await
        {
            return self
                .rollback_group(&group, previous, &candidate, transition, report, problem)
                .await;
        }
        if let Some(committed) = committed {
            committed.store(true, Ordering::Release);
        }
        self.commit_group(
            group,
            specification,
            previous,
            candidate,
            transition,
            report,
        )
        .await
    }

    /// SW-7/SW-8：受影响实例稳定后进入确认期，失败交回替换事务恢复原快照。
    async fn accept_group(
        &self,
        candidate: &GroupGeneration,
        transition: &Transition,
        report: &mut ReplaceReport,
    ) -> Result<()> {
        let changed = self
            .stabilize(&transition.before, &transition.names)
            .await?;
        report.affected_instances = changed.len();
        self.confirm(&changed, &candidate.connection).await
    }

    /// SW-9：确认成功后保存回滚快照并退役旧进程，回收失败交给事务调用方。
    async fn commit_group(
        &self,
        group: Arc<Group>,
        specification: GroupSpec,
        previous: Arc<GroupGeneration>,
        candidate: Arc<GroupGeneration>,
        transition: Transition,
        mut report: ReplaceReport,
    ) -> Result<ReplaceReport> {
        *group.specification.lock() = Some(specification);
        *group.health.lock() = Health {
            state: "running".into(),
            error: None,
        };
        group.restarts.store(0, Ordering::Release);
        report.committed = true;
        self.retain_replacement_states(&group.name, &transition.names, &transition.frozen);
        self.0
            .catalog
            .lock()
            .reports
            .insert(group.name.clone(), report.clone());
        drop(transition);
        let completion = async {
            self.monitor(group.clone(), candidate)?;
            self.retire(&previous).await
        }
        .await;
        if let Err(problem) = completion {
            report.failure = Some(Failure::from(&problem));
            group.health.lock().error = Some(saved_error(&problem));
            self.0
                .catalog
                .lock()
                .reports
                .insert(group.name.clone(), report);
            return Err(problem);
        }
        report.succeeded = true;
        self.0
            .catalog
            .lock()
            .reports
            .insert(group.name.clone(), report.clone());
        Ok(report)
    }

    /// DOC-11 §7：原快照先于旧定义恢复，候选连接在回滚结果记录后退役。
    /// 双重失败分别保留，其他分组继续由各自生命周期管理。
    async fn rollback_group(
        &self,
        group: &Group,
        previous: Arc<GroupGeneration>,
        candidate: &GroupGeneration,
        transition: Transition,
        mut report: ReplaceReport,
        problem: WireError,
    ) -> Result<ReplaceReport> {
        self.inherit(&transition.inherited);
        self.activate(group, previous, None)?;
        let rollback = self.stabilize(&transition.before, &transition.names).await;
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
        drop(transition);
        self.retire(candidate).await?;
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
        Err(problem)
    }

    /// SW-4：候选覆盖当前组件和被使用的接口版本；预检保持既有纪元与绑定。
    pub(super) async fn preflight(&self, group: &Group, candidate: &GroupGeneration) -> Result<()> {
        let current_version = group.current.lock().clone();
        for instance in self.group_instances(&group.name) {
            let key = (
                instance.data.plugin.clone(),
                instance.data.component.clone(),
            );
            let component = candidate
                .components
                .get(&key)
                .ok_or_else(|| error(3, "候选进程缺少正在使用的组件"))?;
            if current_version
                .components
                .get(&key)
                .is_some_and(|old| Arc::ptr_eq(old, component))
            {
                continue;
            }
            let config = instance.data.config.lock().clone();
            self.resolve(component, &config).await?;
        }
        self.preflight_interfaces(group, candidate)
    }

    /// SW-4、ADR-0016：当前绑定确定实际版本需求，候选提供声明覆盖这些需求后通过预检。
    fn preflight_interfaces(&self, group: &Group, candidate: &GroupGeneration) -> Result<()> {
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

    /// SW-6：先切换核心定义目录，再由Registry驱动选定插件或整组的依赖重建。
    pub(super) fn activate(
        &self,
        group: &Group,
        candidate: Arc<GroupGeneration>,
        plugin: Option<&str>,
    ) -> Result<()> {
        let previous = std::mem::replace(
            &mut *group.definitions.lock(),
            candidate.definitions.clone(),
        );
        *group.current.lock() = candidate.clone();
        for (key, old) in previous {
            if plugin.is_some_and(|plugin| key.0 != plugin) {
                continue;
            }
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

    pub(super) fn epochs(&self) -> HashMap<String, cordis::Epoch> {
        self.0
            .catalog
            .lock()
            .instances
            .iter()
            .map(|(name, instance)| (name.clone(), instance.fiber.epoch()))
            .collect()
    }

    pub(super) fn group_instances(&self, group: &str) -> Vec<Arc<Instance>> {
        self.0
            .catalog
            .lock()
            .instances
            .values()
            .filter(|instance| instance.data.group == group)
            .cloned()
            .collect()
    }

    pub(super) fn inherit(&self, states: &HashMap<String, Option<payload::State>>) {
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

    /// SW-7：等待显式替换及纪元变化的纤程完成；错误优先于Active状态检查。
    pub(super) async fn stabilize(
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

    /// SW-8：在确认期观察受影响纤程及连接，故障交回替换事务执行回滚。
    pub(super) async fn confirm(&self, fibers: &[Fiber], connection: &Connection) -> Result<()> {
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

    /// CP-6：连接断开只提交恢复请求，恢复动作由唯一事务执行者安排。
    fn monitor(&self, group: Arc<Group>, version: Arc<GroupGeneration>) -> Result<()> {
        let connection = version.connection.clone();
        let weak = Arc::downgrade(&self.0);
        let mut stopped = self.0.stop.subscribe();
        self.0
            .context
            .effect_async(
                format!("connection-monitor:{}", connection.id()),
                move |_| async move {
                    tokio::select! {
                        reason = connection.closed() => {
                            if let Some(core) = weak.upgrade() {
                                let manager = Manager(core);
                                manager.reclaim_connection(connection.id());
                                let current = group.current.lock().clone();
                                if current.connection.id() == connection.id() {
                                    manager.queue_recovery(group, current, reason).await;
                                }
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

    /// CP-6：确认断开的仍为当前连接后撤回服务，按有限退避重启并继承最近检查点。
    /// 关闭信号中断退避，耗尽次数保留failed；候选完成重建后才报告running。
    async fn recover(
        &self,
        group: Arc<Group>,
        version: Arc<GroupGeneration>,
        reason: String,
    ) -> Result<()> {
        let registered = self
            .0
            .catalog
            .lock()
            .groups
            .get(&group.name)
            .is_some_and(|current| Arc::ptr_eq(current, &group));
        if !registered
            || group.current.lock().connection.id() != version.connection.id()
            || self.0.stopping.load(Ordering::Acquire)
            || self.0.exported.load(Ordering::Acquire)
        {
            return Ok(());
        }
        let problem = error(4, metadata::clipped(&reason));
        self.withdraw(&group, &version, problem).await;
        let specification = group.specification.lock().clone();
        let Some(specification) = specification else {
            return Ok(());
        };
        self.restart_group(group, specification).await
    }

    /// CP-6：退避与重启共用五次以内的尝试预算，关闭信号结束等待，失败保留诊断。
    async fn restart_group(&self, group: Arc<Group>, specification: GroupSpec) -> Result<()> {
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
            if !self.restore_candidate(&group, &candidate).await? {
                continue;
            }
            group.health.lock().state = "running".into();
            self.monitor(group, candidate)?;
            return Ok(());
        }
        group.health.lock().state = "failed".into();
        Ok(())
    }

    /// CP-6、DOC-11 §5：将保存状态交给新世代，等待真实纤程稳定并确认连接仍存活。
    async fn restore_candidate(
        &self,
        group: &Group,
        candidate: &Arc<GroupGeneration>,
    ) -> Result<bool> {
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
        self.activate(group, candidate.clone(), None)?;
        let outcomes = join_all(instances.iter().map(|instance| instance.fiber.settled())).await;
        if !candidate.connection.is_alive() {
            self.reclaim_connection(candidate.connection.id());
            self.withdraw(
                group,
                candidate,
                error(4, candidate.connection.closed().await),
            )
            .await;
            return Ok(false);
        }
        if let Some(problem) = outcomes.into_iter().find_map(std::result::Result::err) {
            group.health.lock().error = Some(saved_error(&kernel_error(problem)));
        }
        Ok(true)
    }

    /// CP-6：以失败定义触发内核正常卸载，依赖方随绑定撤回进入Pending。
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

    /// CP-6、DOC-16 §6：连接退役后回收其路由和控制编号，迟到释放沿各表的幂等规则处理。
    fn reclaim_connection(&self, connection: u64) {
        self.reclaim_routes(connection);
        {
            let mut effects = self.0.effects.lock();
            effects.records.retain(|(owner, _), _| *owner != connection);
            effects.next.remove(&connection);
        }
        self.0.metadata.lock().remove(&connection);
    }

    /// SW-9、CP-7：等待进程拥有者完成退出，再回收连接目录。
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

    /// DOC-11 §9：关闭新请求入口，取消排队操作，等待已开始事务和组清理完成。
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

    /// DOC-11 §9、CP-7：冻结最终状态后释放实例与分组；清理覆盖全部已登记来源。
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
        let exported = self.0.shutdown_export.lock().take();
        let frozen = match exported {
            Some(frozen) => frozen,
            None => self.export(&names, false).await,
        };
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
