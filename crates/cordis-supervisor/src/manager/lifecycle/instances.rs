//! DOC-11 §5.2、DOC-15 §6.4/§11.2：实例挂载、子实例归属、更新、销毁与目录查询。
use super::*;

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
    pub(in crate::manager) async fn install_child(
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
    pub(in crate::manager) fn install_instance(
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

    pub(in crate::manager) fn group_instances(&self, group: &str) -> Vec<Arc<Instance>> {
        self.0
            .catalog
            .lock()
            .instances
            .values()
            .filter(|instance| instance.data.group == group)
            .cloned()
            .collect()
    }
}
