//! DOC-11 SW-1–SW-9、DOC-15 CP-4/CP-6/CP-7：核心持有分组与实例的生命周期决策。
//! 事务在有界工作队列中执行，等待者取消后仍完成提交或回滚；失败清理覆盖候选资源。
use super::*;
use cordis_routing::description::{self, validate_name};
use futures_util::{
    future::join_all,
    stream::{FuturesUnordered, StreamExt},
};
use generation::{Binding, Configuration, Exporter};

/// SW-5：原状态与导出器持有关系；提交或解冻完成后统一释放。
#[derive(Default)]
pub(super) struct Frozen {
    pub(super) states: HashMap<String, Arc<payload::State>>,
    pub(super) stateless: HashSet<String>,
    pub(super) held: Vec<(Arc<Generation>, Arc<Exporter>)>,
    pub(super) failure: Option<WireError>,
}

/// ADR-0041：状态与冻结许可共享寿命；最后一个接收者归还许可时由管理队列解冻。
#[derive(Clone)]
pub struct FrozenSnapshot(Arc<SnapshotLease>);

struct SnapshotLease {
    frozen: Arc<Frozen>,
    owner: Weak<Core>,
    returned: AtomicBool,
}

impl Drop for SnapshotLease {
    fn drop(&mut self) {
        if self.returned.load(Ordering::Acquire) {
            return;
        }
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        // Shutdown consumes every outstanding freeze by retiring its instance.
        if owner.stopping.load(Ordering::Acquire) {
            return;
        }
        if owner.commands.lock().sender.is_closed() {
            // A failed worker cannot accept cleanup. The connection owner closes
            // the affected instances instead of leaving their writers paused.
            for (_, exporter) in &self.frozen.held {
                exporter.object.peer().terminate("冻结快照的管理任务已结束");
            }
            return;
        }
        let mut returned = owner.abandoned_snapshots.lock();
        // Synchronize with the final shutdown drain: a late return either joins
        // that drain or observes retirement already owning the freeze.
        if owner.stopping.load(Ordering::Acquire) {
            return;
        }
        returned.push(self.frozen.clone());
        drop(returned);
        owner.snapshot_returned.notify_one();
    }
}

impl FrozenSnapshot {
    pub fn states(&self) -> &HashMap<String, Arc<payload::State>> {
        &self.0.frozen.states
    }
}

/// SW-5–SW-8：一次替换的原快照、受影响实例名与变更前纪元。
#[derive(Default)]
pub(super) struct Transition {
    pub frozen: Frozen,
    pub inherited: HashMap<String, Option<Arc<payload::State>>>,
    pub names: Vec<String>,
    pub before: HashMap<String, cordis::Epoch>,
    pub permit_pending: bool,
    pub confirming: Option<(std::time::Instant, Vec<String>)>,
}

impl Manager {
    /// ADR-0041：取得具体加载代的父视图，排队后的子挂载继续验证同一代的有效性。
    pub fn active_instance_context(&self, name: &str) -> Result<Context> {
        let instance = self.instance(name)?;
        let generation = instance
            .data
            .current
            .lock()
            .upgrade()
            .ok_or_else(|| error(4, "父实例当前加载缺席"))?;
        generation.active()?;
        Ok(generation.context.clone())
    }
    /// DOC-11 SW-5、ADR-0041：取得指定页面的冻结快照，导出失败解冻已访问的对象。
    pub async fn freeze_instances(&self, names: &[String]) -> Result<FrozenSnapshot> {
        let names = names.to_vec();
        self.transaction(move |manager| async move {
            manager.running()?;
            for name in &names {
                manager.instance(name)?;
            }
            let frozen = manager.export(&names, true).await;
            if let Some(problem) = &frozen.failure {
                let problem = problem.clone();
                manager.thaw(&frozen).await?;
                return Err(problem);
            }
            Ok(FrozenSnapshot(Arc::new(SnapshotLease {
                frozen: Arc::new(frozen),
                owner: Arc::downgrade(&manager.0),
                returned: AtomicBool::new(false),
            })))
        })
        .await
    }

    /// DOC-11 SW-5：回退决议解冻原导出器，成功路径在旧页面清理后释放快照句柄。
    pub async fn thaw_snapshot(&self, snapshot: &FrozenSnapshot) -> Result<()> {
        if !Weak::ptr_eq(&snapshot.0.owner, &Arc::downgrade(&self.0)) {
            return Err(error(2, "冻结快照属于其他管理器"));
        }
        let lease = snapshot.0.clone();
        self.transaction(move |manager| async move {
            if lease.returned.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            manager.thaw_snapshot_contents(&lease.frozen).await
        })
        .await
    }

    async fn thaw_snapshot_contents(&self, frozen: &Frozen) -> Result<()> {
        // Successful replacement consumes the old generation's freeze. Only a
        // surviving generation needs its write permission returned.
        let live = Frozen {
            held: frozen
                .held
                .iter()
                .filter(|(generation, _)| generation.is_current())
                .cloned()
                .collect(),
            ..Frozen::default()
        };
        self.thaw(&live).await
    }

    pub(super) async fn thaw_abandoned_snapshots(&self) {
        let returned = std::mem::take(&mut *self.0.abandoned_snapshots.lock());
        for frozen in returned {
            if let Err(problem) = self.thaw_snapshot_contents(&frozen).await {
                self.0
                    .context
                    .logger()
                    .error(saved_error(&problem).to_string());
            }
        }
    }

    /// SW-5：取得可逆冻结与原快照；任一导出失败时解冻已经访问的导出器。
    pub(super) async fn prepare_transition(
        &self,
        roots: impl IntoIterator<Item = String>,
        incoming: Vec<(Context, Arc<RemoteComponent>, String)>,
        stop_on_error: bool,
    ) -> Result<Transition> {
        let instances = self.select_instances(roots, incoming);
        let names: Vec<_> = instances
            .iter()
            .map(|instance| instance.data.name.clone())
            .collect();
        let mut frozen = self.export(&names, stop_on_error).await;
        if let Some(problem) = &frozen.failure
            && stop_on_error
        {
            let problem = problem.clone();
            self.thaw(&frozen).await?;
            return Err(problem);
        }
        let inherited = {
            let mut catalog = self.0.catalog.lock();
            for name in &frozen.stateless {
                catalog.checkpoints.remove(name);
            }
            catalog.checkpoints.extend(frozen.states.drain());
            instances
                .iter()
                .map(|instance| {
                    (
                        instance.data.name.clone(),
                        catalog
                            .checkpoints
                            .get(&instance.data.name)
                            .cloned()
                            .or_else(|| instance.data.inherited.lock().clone()),
                    )
                })
                .collect()
        };
        Ok(Transition {
            frozen,
            inherited,
            names,
            before: instances
                .iter()
                .map(|instance| (instance.data.name.clone(), instance.fiber.epoch()))
                .collect(),
            permit_pending: false,
            confirming: None,
        })
    }
    /// CP-1、CP-6：准备独立进程并握手，接受其描述符后建立核心分组。
    pub async fn attach_group(&self, specification: GroupSpec) -> Result<GroupSnapshot> {
        self.transaction(move |manager| async move {
            validate_name(&specification.name)?;
            if manager
                .0
                .catalog
                .lock()
                .groups
                .contains_key(&specification.name)
            {
                return Err(error(2, "连接分组已经存在"));
            }
            let name = specification.name.clone();
            manager
                .restore_declared(
                    vec![BatchReplacement::Process(specification)],
                    vec![],
                    HashMap::new(),
                )
                .await?;
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
        specification: Option<LaunchPlan>,
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
    pub(super) async fn prepare_launch(
        &self,
        specification: GroupSpec,
    ) -> Result<(Arc<GroupGeneration>, LaunchPlan)> {
        let generation = self.prepare_process(&specification).await?;
        match self.launch_plan(specification, &generation).await {
            Ok(plan) => Ok((generation, plan)),
            Err(problem) => {
                self.retire(&generation).await?;
                Err(problem)
            }
        }
    }

    /// CP-4、CP-7：初始连接的错误由进程拥有者统一退出和回收。
    pub(super) async fn prepare_process(
        &self,
        specification: &GroupSpec,
    ) -> Result<Arc<GroupGeneration>> {
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
        let mut components = HashMap::new();
        let mut definitions = HashMap::new();
        for (plugin, info) in description::components(&descriptor)? {
            let key = (plugin.name.clone(), info.name.clone());
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
        let description::ComponentTypes {
            inject,
            provides,
            bytes,
        } = description::component_types(group, plugin, info)?;
        let retained = self.retain_metadata(connection, bytes)?;
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
            let definition = manager
                .mount_definition(&specification, inherited.as_ref())
                .await?;
            manager.install_instance(view, specification, definition, inherited.map(Arc::new))
        })
        .await
    }

    /// DOC-15 §11.2、ADR-0041：页面组件挂在活跃后端实例下，父卸载由内核等待子级清理。
    pub async fn mount_child_instance(
        &self,
        parent: String,
        specification: InstanceSpec,
        inherited: Option<payload::State>,
        options: payload::ContextOptions,
    ) -> Result<Fiber> {
        self.transaction(move |manager| async move {
            manager.running()?;
            let parent = manager
                .instance(&parent)?
                .data
                .current
                .lock()
                .upgrade()
                .ok_or_else(|| error(4, "父实例当前加载缺席"))?;
            parent.active()?;
            let registration = parent.reserve()?;
            let view = manager
                .derive_context(&parent.context, &options)?
                .extend(registration);
            let definition = manager
                .mount_definition(&specification, inherited.as_ref())
                .await?;
            parent.active()?;
            manager
                .install_child(
                    &parent.context,
                    view,
                    specification,
                    definition,
                    inherited.map(Arc::new),
                )
                .await
        })
        .await
    }

    /// DOC-15 §11.2、ADR-0041：纯前端 Owner 由同一 Manager.context 创建，页面归其实际父效应。
    pub async fn mount_owned_child(
        &self,
        parent: Context,
        specification: InstanceSpec,
        inherited: Option<Arc<payload::State>>,
        options: payload::ContextOptions,
    ) -> Result<Fiber> {
        self.submit(
            move |manager| async move {
                manager.running()?;
                if !parent.is_current() {
                    return Err(error(4, "页面 Owner 已进入清理"));
                }
                let view = manager.derive_context(&parent, &options)?;
                let definition = manager
                    .mount_definition(&specification, inherited.as_deref())
                    .await?;
                manager
                    .install_child(&parent, view, specification, definition, inherited)
                    .await
            },
            true,
        )
        .await
    }

    /// ADR-0041：两类父对象共用子实例安装与目录清理登记。
    pub(super) async fn install_child(
        &self,
        parent: &Context,
        view: Context,
        specification: InstanceSpec,
        definition: ComponentDef,
        inherited: Option<Arc<payload::State>>,
    ) -> Result<Fiber> {
        let name = specification.name.clone();
        let child = self.install_instance(view, specification, definition, inherited)?;
        if let Err(cause) = self.track_child(parent, name.clone(), child.clone()) {
            self.dispose_owned_child(&name, &child).await?;
            return Err(cause);
        }
        Ok(child)
    }

    /// DOC-15 §11.2：父效应等待子实例回退后同步移除目录与检查点记录。
    fn track_child(&self, parent: &Context, name: String, child: Fiber) -> Result<()> {
        let core = Arc::downgrade(&self.0);
        parent
            .effect("owned-child", move |scope| {
                scope.defer_async(move || async move {
                    let result = child.dispose().await;
                    if let Some(core) = core.upgrade() {
                        Manager(core).remove_owned_child(&name, &child);
                    }
                    result
                })
            })
            .map_err(kernel_error)?;
        Ok(())
    }

    /// DOC-15 §11.2：核心生命周期回调直接等待指定页面，目录移除按纤程身份匹配。
    pub async fn dispose_owned_child(&self, name: &str, expected: &Fiber) -> Result<()> {
        let result = expected.dispose().await.map_err(kernel_error);
        self.remove_owned_child(name, expected);
        result
    }

    fn remove_owned_child(&self, name: &str, expected: &Fiber) {
        let discarded = {
            let mut catalog = self.0.catalog.lock();
            if catalog
                .instances
                .get(name)
                .is_some_and(|instance| instance.fiber == *expected)
            {
                catalog.checkpoints.remove(name);
                catalog.instances.remove(name)
            } else {
                None
            }
        };
        drop(discarded);
    }

    /// DOC-11 §5.2：两种挂载入口共用目标描述符、状态和远端配置的边界检查。
    async fn mount_definition(
        &self,
        specification: &InstanceSpec,
        inherited: Option<&payload::State>,
    ) -> Result<ComponentDef> {
        validate_name(&specification.name)?;
        if self
            .0
            .catalog
            .lock()
            .instances
            .contains_key(&specification.name)
        {
            return Err(error(2, "实例名称已经存在"));
        }
        if let Some(state) = inherited {
            payload::persistent_value(&state.data)
                .map_err(|problem| error(2, problem.to_string()))?;
        }
        let group = self.group(&specification.group)?;
        let current = group.current.lock().clone();
        let key = (
            specification.plugin.clone(),
            specification.component.clone(),
        );
        let component = current
            .components
            .get(&key)
            .ok_or_else(|| error(2, "描述符中缺少目标组件"))?;
        self.resolve(component, &specification.config).await?;
        group
            .definitions
            .lock()
            .get(&key)
            .cloned()
            .ok_or_else(|| error(4, "组件定义已退役"))
    }

    /// DOC-11 §5.2、T-LIF-1：已验证的配置进入内核，目录记录和检查点归属同一个实例名。
    pub(super) fn install_instance(
        &self,
        view: Context,
        specification: InstanceSpec,
        definition: ComponentDef,
        inherited: Option<Arc<payload::State>>,
    ) -> Result<Fiber> {
        let config = Arc::new(specification.config);
        let data = Arc::new(InstanceData {
            name: specification.name.clone(),
            group: specification.group,
            plugin: specification.plugin,
            component: specification.component,
            current: Mutex::new(Weak::new()),
            inherited: Mutex::new(inherited.clone()),
            error: Mutex::new(None),
        });
        let fiber = view
            .mount_with(
                &definition,
                Configuration {
                    instance: data.clone(),
                    config,
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
            manager
                .change_running_group(&instance.data.group, None, Some((name, config)))
                .await
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
        let snapshots = self.0.catalog.lock().checkpoints.clone();
        snapshots
            .into_iter()
            .map(|(name, state)| (name, Arc::unwrap_or_clone(state)))
            .collect()
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
            manager.replace_plan(&group, None).await;
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
            Ok(manager
                .checkpoint_selected(&names)
                .await
                .states
                .into_iter()
                .map(|(name, state)| (name, Arc::unwrap_or_clone(state)))
                .collect())
        })
        .await
    }

    /// DOC-11 §9、CP-11：停止结构变更后保存各实例成功导出的状态，冻结持有至 shutdown。
    pub async fn export_shutdown(&self) -> Result<HashMap<String, Arc<payload::State>>> {
        self.transaction(|manager| async move {
            if let Some(frozen) = manager.0.shutdown_export.lock().as_ref() {
                return Ok(frozen.states.clone());
            }
            manager.running()?;
            manager.0.exported.store(true, Ordering::Release);
            let names = manager.instance_names();
            let mut frozen = manager.export(&names, false).await;
            let durable: HashSet<_> = manager.deployment_instance_names().into_iter().collect();
            frozen.states.retain(|name, _| durable.contains(name));
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
            let names = manager.instance_names();
            Ok(manager
                .checkpoint_selected(&names)
                .await
                .states
                .into_iter()
                .map(|(name, state)| (name, Arc::unwrap_or_clone(state)))
                .collect())
        })
        .await
    }

    /// 部署提交区分已导出的值、已确认无状态的实例和本次缺席的结果。
    pub async fn deployment_checkpoint(
        &self,
        names: Vec<String>,
    ) -> Result<HashMap<String, Option<payload::State>>> {
        self.transaction(move |manager| async move {
            manager.running()?;
            Ok(manager
                .capture_deployment(&names)
                .await
                .into_iter()
                .map(|(name, state)| (name, state.map(Arc::unwrap_or_clone)))
                .collect())
        })
        .await
    }

    pub(super) async fn capture_deployment(
        &self,
        names: &[String],
    ) -> HashMap<String, Option<Arc<payload::State>>> {
        let frozen = self.checkpoint_selected(names).await;
        frozen
            .states
            .into_iter()
            .map(|(name, state)| (name, Some(state)))
            .chain(frozen.stateless.into_iter().map(|name| (name, None)))
            .collect()
    }

    pub fn instance_names(&self) -> Vec<String> {
        self.0.catalog.lock().instances.keys().cloned().collect()
    }

    /// 根部署拥有稳定检查点名。页面和组件创建的子实例由实际父纤程管理。
    pub fn deployment_instance_names(&self) -> Vec<String> {
        self.instances()
            .into_iter()
            .filter(|instance| {
                instance
                    .fiber
                    .parent_context()
                    .is_some_and(|view| view.fiber() == self.0.fiber)
            })
            .map(|instance| instance.data.name.clone())
            .collect()
    }

    /// 周期任务每次只预约一个实例。轮到该实例时，它可能已被管理事务移除。
    pub async fn checkpoint_instance(
        &self,
        name: String,
    ) -> Result<HashMap<String, Arc<payload::State>>> {
        self.transaction(move |manager| async move {
            manager.running()?;
            Ok(manager.checkpoint_selected(&[name]).await.states)
        })
        .await
    }

    async fn checkpoint_selected(&self, names: &[String]) -> Frozen {
        let mut selected = Frozen::default();
        for name in names {
            let frozen = self.export(std::slice::from_ref(name), false).await;
            if let Err(problem) = self.thaw(&frozen).await {
                self.0
                    .context
                    .logger()
                    .error(format!("检查点解冻失败：{problem}"));
            }
            selected.states.extend(frozen.states);
            selected.stateless.extend(frozen.stateless);
        }
        self.0.catalog.lock().checkpoints.extend(
            selected
                .states
                .iter()
                .map(|(name, state)| (name.clone(), state.clone())),
        );
        selected
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
        let mut frozen = Frozen::default();
        for instance in instances {
            let generation = instance.data.current.lock().upgrade();
            let Some(generation) = generation else {
                continue;
            };
            // CP-6：断连可先于排队的回收事务，导出资格同时取决于实际连接。
            if !generation.is_current()
                || instance.fiber.state() != cordis::FiberState::Active
                || !generation.component.connection.is_alive()
            {
                continue;
            }
            let exporter = generation.exporter.lock().clone();
            let Some(exporter) = exporter else {
                frozen.stateless.insert(instance.data.name.clone());
                continue;
            };
            frozen.held.push((generation.clone(), exporter.clone()));
            match self.export_state(&exporter).await {
                Ok(state) => {
                    frozen
                        .states
                        .insert(instance.data.name.clone(), Arc::new(state));
                }
                Err(problem) => {
                    self.0.context.logger().error(format!(
                        "实例 {} 导出失败：{}",
                        instance.data.name,
                        saved_error(&problem)
                    ));
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

    /// 整组重启使用当前实例声明和新的宿主启动意图。
    pub async fn replace_group(&self, specification: GroupSpec) -> Result<ReplaceReport> {
        self.transaction(move |manager| async move {
            manager.replace_process_group(specification).await
        })
        .await
    }

    pub(super) async fn replace_process_group(
        &self,
        specification: GroupSpec,
    ) -> Result<ReplaceReport> {
        let group = specification.name.clone();
        Box::pin(self.change_running_group(
            &group,
            Some(BatchReplacement::Process(specification)),
            None,
        ))
        .await?;
        self.0
            .catalog
            .lock()
            .reports
            .get(&group)
            .cloned()
            .ok_or_else(|| error(4, "分组换代报告缺席"))
    }

    /// SW-4：候选覆盖当前组件和被使用的接口版本；预检保持既有纪元与绑定。
    /// SW-4、ADR-0016：当前绑定确定实际版本需求，候选提供声明覆盖这些需求后通过预检。
    pub(super) fn preflight_interfaces(
        &self,
        group: &Group,
        candidate: &GroupGeneration,
    ) -> Result<()> {
        let instances = self.instances();
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
    pub(super) fn activate(&self, group: &Group, candidate: Arc<GroupGeneration>) -> Result<()> {
        let previous = std::mem::replace(
            &mut *group.definitions.lock(),
            candidate.definitions.clone(),
        );
        *group.current.lock() = candidate.clone();
        for (key, old) in previous {
            match candidate.definitions.get(&key) {
                Some(next) => {
                    if old.id() != next.id() {
                        self.0.context.registry().replace(&old, next);
                    }
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

    pub(super) fn inherit(&self, states: &HashMap<String, Option<Arc<payload::State>>>) {
        let instances = {
            let mut catalog = self.0.catalog.lock();
            let mut instances = Vec::new();
            for (name, state) in states {
                match state {
                    Some(state) => {
                        catalog.checkpoints.insert(name.clone(), state.clone());
                    }
                    None => {
                        catalog.checkpoints.remove(name);
                    }
                }
                if let Some(instance) = catalog.instances.get(name) {
                    instances.push((instance.clone(), state.clone()));
                }
            }
            instances
        };
        for (instance, state) in instances {
            *instance.data.inherited.lock() = state;
        }
    }

    /// ADR-0044：配置调和接受内核计算的 Pending，版本替换继续要求 Active。
    pub(super) async fn stabilize_with(
        &self,
        before: &HashMap<String, cordis::Epoch>,
        explicit: &[String],
        permit_pending: bool,
    ) -> Result<Vec<Fiber>> {
        let future = async {
            loop {
                let instances = self.instances();
                let mut changed: Vec<_> = instances
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
                // ADR-0041：等待期间父级可完成子实例退役，按当前目录身份判断存活部署。
                changed.retain(|instance| {
                    let catalog = self.0.catalog.lock();
                    catalog
                        .instances
                        .get(&instance.data.name)
                        .is_some_and(|current| Arc::ptr_eq(current, instance))
                });
                if changed.iter().any(|instance| !instance.fiber.is_settled()) {
                    continue;
                }
                if changed.iter().any(|instance| {
                    let state = instance.fiber.state();
                    state != cordis::FiberState::Active
                        && !(permit_pending && state == cordis::FiberState::Pending)
                }) {
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
    pub(super) async fn confirm(&self, fibers: &[Fiber], connections: &[Connection]) -> Result<()> {
        if connections.is_empty() || fibers.is_empty() || self.0.options.confirm.is_zero() {
            return Ok(());
        }
        let failed = Arc::new(tokio::sync::Notify::new());
        let observed = failed.clone();
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
                    observed.notify_one();
                }
                None
            })
            .map_err(kernel_error)?;
        let mut closed: FuturesUnordered<_> = connections.iter().map(Connection::closed).collect();
        let result = tokio::select! {
            _ = tokio::time::sleep(self.0.options.confirm) => Ok(()),
            _ = failed.notified() => Err(error(4, "确认期内实例离开Active")),
            Some(reason) = closed.next() => Err(error(4, reason)),
        };
        listener.dispose().await.map_err(kernel_error)?;
        result
    }

    /// CP-6：连接断开只提交恢复请求，恢复动作由唯一事务执行者安排。
    pub(super) fn monitor(&self, group: Arc<Group>, version: Arc<GroupGeneration>) -> Result<()> {
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
        if !self.recovery_current(&group, version.connection.id()) {
            return Ok(());
        }
        let problem = error(4, metadata::clipped(&reason));
        let transition = self
            .prepare_transition(
                self.group_instances(&group.name)
                    .into_iter()
                    .map(|i| i.data.name.clone()),
                Vec::new(),
                false,
            )
            .await?;
        self.inherit(&transition.inherited);
        self.withdraw(&group, &version, problem).await;
        let cohort = transition
            .names
            .iter()
            .filter_map(|name| self.find_instance(name))
            .map(|instance| Arc::downgrade(&instance))
            .collect();
        self.thaw_current(&transition.frozen).await?;
        self.schedule_restart(group, cohort);
        Ok(())
    }

    fn recovery_current(&self, group: &Arc<Group>, connection: u64) -> bool {
        self.running().is_ok()
            && self
                .0
                .catalog
                .lock()
                .groups
                .get(&group.name)
                .is_some_and(|current| Arc::ptr_eq(current, group))
            && group.current.lock().connection.id() == connection
    }

    /// 退避计时在管理队列外；轮到实际恢复时重新核对分组和连接。
    fn schedule_restart(&self, group: Arc<Group>, cohort: Vec<Weak<Instance>>) {
        let Some(specification) = group.specification.lock().clone() else {
            return;
        };
        let attempt = group.restarts.load(Ordering::Acquire);
        if attempt >= specification.restart.max {
            group.health.lock().state = "failed".into();
            return;
        }
        group.health.lock().state = "backoff".into();
        let delay = specification
            .restart
            .backoff
            .saturating_mul(1u32 << attempt);
        let connection = group.current.lock().connection.id();
        let mut shutdown = self.0.stop.subscribe();
        let sender = self.0.commands.lock().sender.clone();
        let (reply, _result) = oneshot::channel();
        let work: Command = Box::new(Action {
            callback: move |manager: Manager| async move {
                if manager.recovery_current(&group, connection) {
                    group.restarts.fetch_add(1, Ordering::AcqRel);
                    group.health.lock().state = "restarting".into();
                    if let Err(problem) = manager
                        .retry_deployment(specification.specification, &cohort)
                        .await
                    {
                        group.health.lock().error = Some(saved_error(&problem));
                        manager.schedule_restart(group, cohort);
                    }
                }
                Ok(())
            },
            reply,
        });
        tokio::spawn(async move {
            if *shutdown.borrow() {
                return;
            }
            tokio::select! {
                _ = shutdown.changed() => {},
                _ = tokio::time::sleep(delay) => { let _ = sender.send(work).await; },
            }
        });
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
        self.0.metadata.reclaim(connection);
    }

    /// SW-9、CP-7：等待进程拥有者完成退出，再回收连接目录。
    pub(super) async fn retire(&self, version: &GroupGeneration) -> Result<()> {
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
        let exported = self.0.shutdown_export.lock().take();
        let frozen = match exported {
            Some(frozen) => frozen,
            None => {
                let mut frozen = self.export(&self.instance_names(), false).await;
                self.0
                    .catalog
                    .lock()
                    .checkpoints
                    .extend(frozen.states.drain());
                frozen
            }
        };
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
        failure = failure.or(self.close_held_deployment().await.err());
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
            self.replace_plan(&group, None).await;
            group.health.lock().state = "stopped".into();
        }
        let discarded = std::mem::take(&mut self.0.catalog.lock().instances);
        drop(discarded);
        self.0.abandoned_snapshots.lock().clear();
        failure.map_or(Ok(()), Err)
    }
}
