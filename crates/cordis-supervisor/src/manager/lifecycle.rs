//! DOC-11 SW-1–SW-9、DOC-15 CP-4/CP-6/CP-7：核心持有分组与实例的生命周期决策。
//! 事务在有界工作队列中执行，等待者取消后仍完成提交或回滚；失败清理覆盖候选资源。
use super::*;
use cordis_routing::description::{self, validate_name};
use futures_util::{
    future::join_all,
    stream::{FuturesUnordered, StreamExt},
};
use generation::{Binding, Configuration, Exporter};

mod instances;
mod recovery;
mod state;

pub use state::FrozenSnapshot;
pub(super) use state::{Frozen, Transition};

impl Manager {
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
            manager.teardown_group(&group, &mut failure).await;
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

    /// CP-7：注销分组的全部内核定义、退役活动进程并撤回启动计划。
    async fn teardown_group(&self, group: &Group, failure: &mut Option<WireError>) {
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
        self.replace_plan(group, None).await;
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
}
