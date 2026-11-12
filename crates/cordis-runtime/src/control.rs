//! DOC-15 §3.2：串行执行管理变更，并协调运行版本、原快照与部署叠加层。

mod batch;
mod contract;
mod outcomes;
mod sources;
pub(crate) use outcomes::{BatchOutcomes, PendingBatch};
pub(crate) use sources::SourceBindings;

use crate::{
    builtins::{Logs, Management, Permissions},
    catalog::{self, Catalog, Package},
    config::{self, Configuration, InstanceSettings, Overlay, OverlayEntry, Result, parameter},
    install,
    manifest::Entry,
    request::{Handle, Operation, Request},
    status::Diagnostics,
    storage::Store,
};
use cordis::{Context, Fiber, NamedProvision, Realm};
use cordis_supervisor::manager::{InstanceSpec, Manager};
use cordis_up::{Value, WireError, payload};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

struct Mounted {
    settings: InstanceSettings,
    package: Package,
    group: String,
    fiber: Fiber,
    _view: Context,
    manage: Option<NamedProvision>,
}

/// DOC-11 §7：保存一次插件替换涉及的部署版本和显式继承状态。
struct Replacement {
    group: String,
    candidate: Package,
    previous: Package,
    affected: Vec<String>,
    packages: Vec<Package>,
    overlay: Overlay,
    inherited: HashMap<String, Option<payload::State>>,
}

pub struct Controller {
    pub manager: Arc<Manager>,
    pub configuration: Configuration,
    pub path: PathBuf,
    pub project: PathBuf,
    pub plugins: PathBuf,
    pub store: Store,
    pub overlay: Overlay,
    pub diagnostics: Arc<Mutex<Diagnostics>>,
    pub permissions: Permissions,
    pub handle: Handle,
    pub logs: Logs,
    catalog: Catalog,
    mounted: BTreeMap<String, Mounted>,
    groups: BTreeMap<String, Vec<Package>>,
    installed: HashMap<String, Package>,
    outcomes: Arc<Mutex<BatchOutcomes>>,
    sources: Arc<Mutex<SourceBindings>>,
}

/// DOC-15 §3.2：管理器与运行循环共享的诊断、授权、队列和日志入口。
pub struct ControlServices {
    pub diagnostics: Arc<Mutex<Diagnostics>>,
    pub permissions: Permissions,
    pub handle: Handle,
    pub logs: Logs,
}

impl Controller {
    /// DOC-15 §3.2：绑定当前配置、存储及管理队列，实例在 start 中建立。
    pub fn new(
        manager: Arc<Manager>,
        configuration: Configuration,
        path: PathBuf,
        plugins: PathBuf,
        store: Store,
        overlay: Overlay,
        services: ControlServices,
    ) -> Self {
        let ControlServices {
            diagnostics,
            permissions,
            handle,
            logs,
        } = services;
        let project = path.parent().expect("configuration parent").to_owned();
        let outcomes = Arc::new(Mutex::new(BatchOutcomes::new(
            store.directory.join("batches.json"),
            diagnostics.clone(),
        )));
        let sources = Arc::new(Mutex::new(SourceBindings::new(
            store.directory.join("sources.json"),
            plugins
                .strip_prefix(&project)
                .unwrap_or(&plugins)
                .to_owned(),
        )));
        diagnostics.lock().source_bindings = Some(sources.clone());
        Self {
            manager,
            configuration,
            path,
            project,
            plugins,
            store,
            overlay,
            diagnostics,
            permissions,
            handle,
            logs,
            catalog: Catalog::default(),
            mounted: BTreeMap::new(),
            groups: BTreeMap::new(),
            installed: HashMap::new(),
            outcomes,
            sources,
        }
    }

    /// DOC-15 §3.2：发现来源、建立进程分组，再按检查点挂载实例；局部错误记入诊断。
    pub async fn start(&mut self) -> Result<()> {
        self.outcomes.lock().load()?;
        self.sources.lock().load()?;
        if self
            .sources
            .lock()
            .apply_declarations(&self.configuration, &mut self.overlay)
        {
            self.store.save_overlay(&self.overlay)?;
        }
        self.discover().await?;
        let target = self.configuration.effective(&self.overlay)?;
        *self.permissions.lock() = target.clone();
        self.diagnostics.lock().desired = target.clone();
        let groups = self.startup_packages(&target).await?;
        self.start_groups(&target, groups).await;
        for (name, settings) in target {
            if self.diagnostics.lock().instance_errors.contains_key(&name) {
                continue;
            }
            let result = match self.store.load(&name) {
                Ok(inherited) => self.mount(&name, settings, inherited).await,
                Err(problem) => Err(problem),
            };
            if let Err(problem) = result {
                self.instance_error(&name, &problem);
            }
        }
        self.refresh()?;
        Ok(())
    }

    /// DOC-15 §4：相同分组中的相同插件引用共用一个来源，各来源在启动前固定。
    async fn startup_packages(
        &mut self,
        target: &BTreeMap<String, InstanceSettings>,
    ) -> Result<BTreeMap<String, Vec<Package>>> {
        let mut groups: BTreeMap<String, Vec<Package>> = BTreeMap::new();
        for (name, settings) in target {
            match self.instance_package(name, settings).await {
                Ok(package) => {
                    let group = settings
                        .group
                        .clone()
                        .unwrap_or_else(|| package.name.clone());
                    config::instance_name(&group)?;
                    let packages = groups.entry(group).or_default();
                    if !packages
                        .iter()
                        .any(|item| item.reference == package.reference)
                    {
                        packages.push(package);
                    }
                }
                Err(problem) => self.instance_error(name, &problem),
            }
        }
        Ok(groups)
    }

    /// DOC-15 CP-6：分组启动失败归属该组实例，其余组继续启动。
    async fn start_groups(
        &mut self,
        target: &BTreeMap<String, InstanceSettings>,
        groups: BTreeMap<String, Vec<Package>>,
    ) {
        for (group, packages) in groups {
            let plan = catalog::plan(
                &group,
                &packages,
                &self.project,
                &self.configuration.runtime,
            );
            let result = match plan {
                Ok(plan) => self.manager.attach_group(plan).await.map(|_| ()),
                Err(problem) => Err(problem),
            };
            match result {
                Ok(()) => {
                    self.groups.insert(group, packages);
                }
                Err(problem) => {
                    for (name, settings) in target {
                        let desired_group = settings.group.clone().or_else(|| {
                            self.catalog
                                .get(&settings.plugin)
                                .ok()
                                .map(|package| package.name)
                        });
                        if desired_group.as_deref() == Some(group.as_str()) {
                            self.instance_error(name, &problem);
                        }
                    }
                }
            }
        }
    }

    /// DOC-15 §3.2：已授权请求由单一队列按方法分派，具体事务负责持久化。
    pub async fn execute(&mut self, request: &Request) -> Result<Value> {
        self.sources.lock().confirm(&self.manager.snapshot())?;
        if let Operation::ReplaceBatch { request_id, .. } = &request.operation {
            return self.execute_recorded_batch(request, request_id).await;
        }
        self.execute_operation(request).await
    }

    /// DOC-15 §3.2：实际执行发生在管理队列，批次结果由外层记录后提供给调用方。
    async fn execute_operation(&mut self, request: &Request) -> Result<Value> {
        match &request.operation {
            Operation::Install(source) => {
                return self.install_source(&request.caller.plugin, source).await;
            }
            Operation::Add {
                name,
                plugin,
                group,
                config,
            } => {
                self.add(name, plugin, group, config).await?;
            }
            Operation::Remove(name) => {
                self.current(name)?;
                let mut next = self.overlay.clone();
                next.instance.remove(name);
                next.previous.remove(name);
                next.removed.insert(name.clone());
                self.commit_configuration(self.configuration.clone(), next)
                    .await?;
            }
            Operation::Configure { name, config } => {
                let mut settings = self.current(name)?;
                settings.config = config.clone();
                let mut next = self.overlay.clone();
                next.instance
                    .insert(name.clone(), OverlayEntry::from(&settings));
                self.commit_configuration(self.configuration.clone(), next)
                    .await?;
            }
            Operation::Replace { name, plugin } => {
                self.replace(name, plugin, false).await?;
            }
            Operation::ReplaceBatch {
                replacements,
                configuration,
                ..
            } => {
                self.replace_batch(replacements, configuration.clone()?)
                    .await?;
            }
            Operation::Rollback(name) => {
                let previous = self
                    .overlay
                    .previous
                    .get(name)
                    .cloned()
                    .ok_or_else(|| WireError::new(4, "实例的上一版本引用缺席"))?;
                self.replace(name, &previous.plugin, true).await?;
            }
            Operation::Reload(configuration) => {
                self.discover().await?;
                self.commit_configuration(configuration.clone(), self.overlay.clone())
                    .await?;
                self.diagnostics.lock().configuration_error = None;
            }
            Operation::Shutdown => {}
        }
        Ok(Value::Null)
    }

    /// DOC-15 §3.2：安装调用方授权目录中的来源，探测后公布安装引用。
    async fn install_source(&mut self, caller: &str, source: &str) -> Result<Value> {
        let data = self.store.data_dir(caller)?;
        let source = config::resolve_path(&self.project, std::path::Path::new(source));
        let reference = install::install(&source, &self.project, &data, &self.plugins)?;
        let package = catalog::inspect(
            reference.clone(),
            self.plugins.join(&reference),
            &self.project,
            &self.configuration.runtime,
        )
        .await?;
        self.catalog.packages.insert(reference.clone(), package);
        self.refresh()?;
        Ok(reference.into())
    }

    /// DOC-15 §3.2：新增实例沿用人工配置中的授权，配置事务承担挂载及失败恢复。
    async fn add(
        &mut self,
        name: &str,
        plugin: &str,
        group: &Option<String>,
        config: &toml::Table,
    ) -> Result<()> {
        config::instance_name(name)?;
        if self.configuration.entries(&self.overlay).contains_key(name) {
            return Err(parameter("实例名称已经存在"));
        }
        let settings = InstanceSettings {
            plugin: plugin.into(),
            component: None,
            group: group.clone(),
            scope: None,
            disabled: false,
            config: config.clone(),
            isolate: BTreeMap::new(),
            intercept: BTreeMap::new(),
            grants: self
                .configuration
                .instance
                .get(name)
                .map(|item| item.grants.clone())
                .unwrap_or_default(),
            forwarder: false,
        };
        let mut next = self.overlay.clone();
        next.removed.remove(name);
        next.instance
            .insert(name.into(), OverlayEntry::from(&settings));
        self.commit_configuration(self.configuration.clone(), next)
            .await
    }

    /// DOC-15 §3.2：按部署叠加规则取得管理操作的当前实例设置。
    fn current(&self, name: &str) -> Result<InstanceSettings> {
        self.configuration
            .entries(&self.overlay)
            .remove(name)
            .ok_or_else(|| WireError::new(4, format!("实例 {name} 缺席")))
    }

    /// DOC-15 §3.2：重读插件目录并刷新来源诊断，已有安装副本继续可用。
    async fn discover(&mut self) -> Result<()> {
        self.installed.clear();
        let errors = self
            .catalog
            .discover(&self.plugins, &self.project, &self.configuration.runtime)
            .await?;
        self.diagnostics.lock().plugin_errors = errors;
        Ok(())
    }

    /// DOC-15 §3.2、DOC-11 §7：装载前固定来源副本，使上一版本引用持续指向原字节。
    async fn installed_package(&mut self, reference: &str) -> Result<Package> {
        if let Some(package) = self.installed.get(reference) {
            return Ok(package.clone());
        }
        let source = self.catalog.get(reference)?;
        let installed =
            install::install(&source.source, &self.project, &self.project, &self.plugins)?;
        let package = if installed == source.reference {
            source
        } else {
            catalog::inspect(
                installed.clone(),
                self.plugins.join(&installed),
                &self.project,
                &self.configuration.runtime,
            )
            .await?
        };
        self.catalog.packages.insert(installed, package.clone());
        self.installed.insert(reference.into(), package.clone());
        Ok(package)
    }

    /// DOC-11 §10：声明保持时恢复该实例最后确认的副本，管理覆盖引用保持其提交优先级。
    async fn instance_package(
        &mut self,
        name: &str,
        settings: &InstanceSettings,
    ) -> Result<Package> {
        let declared = self
            .configuration
            .instance
            .get(name)
            .map(|item| item.plugin.as_str())
            .unwrap_or(&settings.plugin);
        let reference = self.sources.lock().select(name, declared, &settings.plugin);
        self.installed_package(&reference).await
    }

    /// DOC-15 §3.2：审计记录保留真实调用方、目标和操作结果。
    pub fn audit(&self, request: &Request, result: &Result<Value>) {
        let target = request.operation.target();
        let outcome = match result {
            Ok(_) => "success".into(),
            Err(problem) => format!("error {}: {}", problem.code, problem.message),
        };
        self.logs.emit(
            if result.is_ok() { 2 } else { 0 },
            &request.caller.instance,
            &format!(
                "manage method={} target={} result={outcome}",
                request.operation.method(),
                target
            ),
        );
    }

    /// CP-6：实例局部故障进入状态查询和日志，其余实例继续运行。
    fn instance_error(&self, name: &str, problem: &WireError) {
        self.diagnostics
            .lock()
            .instance_errors
            .insert(name.into(), problem.to_string());
        self.logs.emit(0, name, &problem.to_string());
    }

    /// DOC-15 §3.2：将已采用部署及来源库存提供给只读状态接口。
    fn refresh(&self) -> Result<()> {
        self.sources
            .lock()
            .track(&self.configuration, &self.overlay, &self.mounted);
        let mut diagnostics = self.diagnostics.lock();
        diagnostics.desired = self.configuration.effective(&self.overlay)?;
        diagnostics.overrides = self
            .overlay
            .instance
            .keys()
            .chain(&self.overlay.removed)
            .cloned()
            .collect();
        diagnostics.overrides.sort();
        diagnostics.sources = self.catalog.packages.values().cloned().collect();
        diagnostics.active_references = self
            .mounted
            .iter()
            .map(|(name, item)| (name.clone(), item.package.reference.clone()))
            .collect();
        Ok(())
    }

    /// DOC-15 §4：复用相同来源的分组成员，新增成员通过监管器替换完整分组。
    async fn ensure_group(&mut self, group: &str, package: &Package) -> Result<()> {
        let mut packages = self.groups.get(group).cloned().unwrap_or_default();
        if let Some(existing) = packages.iter().find(|item| item.name == package.name) {
            if existing.reference == package.reference {
                return Ok(());
            }
            return Err(parameter("同组同逻辑插件需要一致的安装引用"));
        }
        packages.push(package.clone());
        let plan = catalog::plan(group, &packages, &self.project, &self.configuration.runtime)?;
        if self.groups.contains_key(group) {
            self.manager.replace_group(plan).await?;
        } else {
            self.manager.attach_group(plan).await?;
        }
        self.groups.insert(group.into(), packages);
        Ok(())
    }

    /// DOC-15 §3.2、ADR-0025：按实例视图挂载远端组件，失败时回收本次管理授权。
    async fn mount(
        &mut self,
        name: &str,
        settings: InstanceSettings,
        inherited: Option<payload::State>,
    ) -> Result<()> {
        let package = self.instance_package(name, &settings).await?;
        let group = settings
            .group
            .clone()
            .unwrap_or_else(|| package.name.clone());
        config::instance_name(&group)?;
        self.ensure_group(&group, &package).await?;
        let component = self.mount_component(name, &group, &package, &settings)?;
        let view = self.mount_view(&settings)?;
        let manage = self.provide_management(name, &settings, &view)?;
        self.diagnostics
            .lock()
            .active_references
            .insert(name.into(), package.reference.clone());
        let result = self
            .manager
            .mount_in(
                view.clone(),
                InstanceSpec {
                    name: name.into(),
                    group: group.clone(),
                    plugin: package.name.clone(),
                    component,
                    config: Value::serialized(&settings.config).map_err(parameter)?,
                },
                inherited,
            )
            .await;
        let fiber = match result {
            Ok(fiber) => fiber,
            Err(problem) => {
                self.diagnostics.lock().active_references.remove(name);
                if let Some(manage) = manage {
                    manage.dispose().await.map_err(kernel)?;
                }
                return Err(problem);
            }
        };
        self.store.consume(name)?;
        self.mounted.insert(
            name.into(),
            Mounted {
                settings,
                package,
                group,
                fiber,
                _view: view,
                manage,
            },
        );
        self.diagnostics.lock().instance_errors.remove(name);
        Ok(())
    }

    /// DOC-15 §8：握手描述符确定组件和版本，缺少管理授权的声明依赖进入等待。
    fn mount_component(
        &mut self,
        name: &str,
        group: &str,
        package: &Package,
        settings: &InstanceSettings,
    ) -> Result<String> {
        let snapshot = self.manager.snapshot();
        let descriptor = snapshot
            .groups
            .iter()
            .find(|item| item.name == group)
            .and_then(|group| {
                group
                    .plugins
                    .iter()
                    .find(|plugin| plugin.name == package.name)
            })
            .ok_or_else(|| WireError::new(4, "分组握手中缺少目标插件"))?;
        if descriptor.version != package.version {
            return Err(parameter("插件清单与进程描述符版本冲突"));
        }
        self.catalog
            .packages
            .get_mut(&package.reference)
            .expect("selected source retained")
            .descriptor = Some(descriptor.clone());
        let component = match &settings.component {
            Some(component) => component.clone(),
            None if descriptor.components.len() == 1 => descriptor.components[0].name.clone(),
            None => descriptor
                .components
                .iter()
                .find(|component| component.name == package.name)
                .map(|component| component.name.clone())
                .ok_or_else(|| parameter("插件包含多个组件，请在配置中指定component"))?,
        };
        if settings
            .grants
            .get(config::MANAGE)
            .is_none_or(Vec::is_empty)
            && descriptor
                .components
                .iter()
                .find(|item| item.name == component)
                .is_some_and(|item| {
                    item.inject
                        .iter()
                        .any(|dependency| dependency.interface.starts_with("cordis.manage@1"))
                })
        {
            self.logs
                .emit(1, name, "实例需要cordis.manage@1授权，管理依赖等待配置授权");
        }
        Ok(component)
    }

    /// ADR-0025：部署隔离域与拦截进入派生视图，管理能力使用实例私有域。
    fn mount_view(&self, settings: &InstanceSettings) -> Result<Context> {
        let isolate = settings
            .isolate
            .iter()
            .map(|(interface, realm)| payload::Isolation {
                interface: interface.clone(),
                realm: match realm {
                    config::Isolation::Named(name) => Some(name.clone()),
                    config::Isolation::Fresh => None,
                },
            })
            .collect::<Vec<_>>();
        let intercept = settings
            .intercept
            .iter()
            .map(|(interface, meta)| {
                Ok(payload::Interception {
                    interface: interface.clone(),
                    meta: Value::serialized(meta).map_err(parameter)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(self
            .manager
            .derive_context(
                &self.manager.context(),
                &payload::ContextOptions { isolate, intercept },
            )?
            .isolate_named(config::MANAGE, Realm::new()))
    }

    /// DOC-15 §3.2：按配置授权提供实例专有管理对象，调用时继续逐方法鉴权。
    fn provide_management(
        &self,
        name: &str,
        settings: &InstanceSettings,
        view: &Context,
    ) -> Result<Option<NamedProvision>> {
        if settings
            .grants
            .get(config::MANAGE)
            .is_none_or(Vec::is_empty)
        {
            return Ok(None);
        }
        self.manager
            .provide_builtin_in(
                view,
                "cordis.manage@1.1",
                config::METHODS
                    .iter()
                    .map(|method| (*method).into())
                    .collect(),
                Arc::new(Management {
                    owner: name.into(),
                    permissions: self.permissions.clone(),
                    control: self.handle.clone(),
                    configuration: self.path.clone(),
                    store: self.store.clone(),
                    diagnostics: self.diagnostics.clone(),
                    outcomes: self.outcomes.clone(),
                    logs: self.logs.clone(),
                }),
            )
            .map(Some)
    }

    /// T-EFF-2、CP-7：释放实例与授权，最后一个成员退出后释放其分组进程。
    async fn remove(&mut self, name: &str) -> Result<()> {
        let Some(mounted) = self.mounted.remove(name) else {
            return Ok(());
        };
        let result = self.manager.dispose(name).await;
        let grant = match mounted.manage {
            Some(grant) => grant.dispose().await.map_err(kernel),
            None => Ok(()),
        };
        result.and(grant)?;
        if !self
            .mounted
            .values()
            .any(|item| item.group == mounted.group)
        {
            self.manager.detach_group(&mounted.group).await?;
            self.groups.remove(&mounted.group);
        }
        Ok(())
    }

    /// loader EntryGroup.update、S-FBR-4：按名称调和变化项，配置更新保留实例身份。
    async fn reconcile(
        &mut self,
        target: &BTreeMap<String, InstanceSettings>,
        inherited: &HashMap<String, payload::State>,
    ) -> Result<()> {
        for name in self.changed_instances(target) {
            self.remove(&name).await?;
        }
        let mut mounted_now = Vec::new();
        for (name, settings) in target {
            if let Some(current) = self.mounted.get_mut(name) {
                if current.settings.config != settings.config {
                    self.manager
                        .update(
                            name,
                            Value::serialized(&settings.config).map_err(parameter)?,
                        )
                        .await?;
                    current.settings = settings.clone();
                }
            } else {
                self.mount(name, settings.clone(), inherited.get(name).cloned())
                    .await?;
                mounted_now.push(name.clone());
            }
        }
        for name in mounted_now {
            self.settle(&name).await?;
        }
        Ok(())
    }

    /// loader Entry.update：组件或上下文变化重建，其余变化交给实例配置更新。
    fn changed_instances(&self, target: &BTreeMap<String, InstanceSettings>) -> Vec<String> {
        self.mounted
            .iter()
            .filter_map(|(name, current)| {
                let next = target.get(name);
                (next.is_none()
                    || next.is_some_and(|next| {
                        next.plugin != current.settings.plugin
                            || next.component != current.settings.component
                            || next.group != current.settings.group
                            || next.isolate != current.settings.isolate
                            || next.intercept != current.settings.intercept
                            || next.grants != current.settings.grants
                    }))
                .then(|| name.clone())
            })
            .collect()
    }

    /// CP-4：实例稳定等待受 apply 截止时间限制，保留内核返回的加载错误。
    async fn settle(&self, name: &str) -> Result<()> {
        let mounted = self.mounted.get(name).expect("mounted instance retained");
        tokio::time::timeout(
            Duration::from_millis(self.configuration.runtime.apply_timeout_ms),
            mounted.fiber.settled(),
        )
        .await
        .map_err(|_| WireError::new(6, "实例加载截止时间到达"))?
        .map_err(kernel)
    }

    /// DOC-15 §3.2：保存变化项原状态，调和成功后发布配置；失败恢复原部署。
    async fn commit_configuration(
        &mut self,
        mut configuration: Configuration,
        mut overlay: Overlay,
    ) -> Result<()> {
        self.sources
            .lock()
            .apply_declarations(&configuration, &mut overlay);
        let before = self.configuration.effective(&self.overlay)?;
        let target = configuration.effective(&overlay)?;
        if configuration.runtime != self.configuration.runtime {
            self.logs
                .emit(1, "runtime", "runtime设置将在下次启动时生效");
            configuration.runtime = self.configuration.runtime.clone();
        }
        let affected = before
            .keys()
            .chain(target.keys())
            .filter(|name| before.get(*name) != target.get(*name))
            .cloned()
            .collect::<BTreeSet<_>>();
        let exported = affected
            .iter()
            .filter(|name| self.mounted.contains_key(*name))
            .cloned()
            .collect::<Vec<_>>();
        let saved = self.manager.checkpoint_instances(&exported).await?;
        let inherited = inherited_configuration(&before, &target, &saved);
        *self.permissions.lock() = target.clone();
        let result = self
            .reconcile(&target, &inherited)
            .await
            .and_then(|_| self.persist_configuration(&before, &target, &overlay));
        if let Err(problem) = result {
            return self
                .restore_configuration(&before, &affected, &saved, problem)
                .await;
        }
        self.configuration = configuration;
        self.overlay = overlay;
        self.refresh()?;
        self.store.retain_replacements(&self.overlay)
    }

    /// DOC-15 §3.2、CP-11：发布新部署前清除移除或更换组件的旧检查点。
    fn persist_configuration(
        &self,
        before: &BTreeMap<String, InstanceSettings>,
        target: &BTreeMap<String, InstanceSettings>,
        overlay: &Overlay,
    ) -> Result<()> {
        for (name, old) in before {
            if target
                .get(name)
                .is_none_or(|next| old.plugin != next.plugin || old.component != next.component)
            {
                self.store.forget(name)?;
            }
        }
        self.store.save_overlay(overlay)
    }

    /// DOC-11 §7：配置事务失败恢复原授权、实例定义和原状态，保留双重失败原因。
    async fn restore_configuration(
        &mut self,
        before: &BTreeMap<String, InstanceSettings>,
        affected: &BTreeSet<String>,
        saved: &HashMap<String, payload::State>,
        problem: WireError,
    ) -> Result<()> {
        *self.permissions.lock() = before.clone();
        for name in affected {
            self.remove(name).await?;
        }
        if let Err(rollback) = self.reconcile(before, saved).await {
            return Err(WireError::new(
                1,
                format!("管理变更失败：{problem}；恢复失败：{rollback}"),
            ));
        }
        self.store.checkpoint(saved)?;
        Err(problem)
    }

    /// DOC-11 SW-5–SW-9：提交后的退役错误仍持久化活动版本，并保留原错误供调用方处理。
    async fn replace(&mut self, name: &str, reference: &str, rollback: bool) -> Result<()> {
        let replacement = self.prepare_replacement(name, reference, rollback).await?;
        if replacement.candidate.reference == replacement.previous.reference && !rollback {
            return Ok(());
        }
        let outcome = self
            .replace_package(
                &replacement.group,
                &replacement.candidate,
                &replacement.packages,
                replacement.inherited.clone(),
            )
            .await;
        if let Err(problem) = &outcome
            && !replacement_committed(problem)
        {
            return outcome;
        }
        let original = self.manager.last_replacement_states(&replacement.group);
        if let Err(problem) = self.persist_replacement(&replacement, &original) {
            return self
                .restore_after_persistence_failure(replacement, original, problem)
                .await;
        }
        self.accept_replacement(replacement)?;
        self.store.retain_replacements(&self.overlay)?;
        outcome
    }

    /// DOC-11 §7：将受影响实例的上一版本和对应原快照组成同一替换请求。
    async fn prepare_replacement(
        &mut self,
        name: &str,
        reference: &str,
        rollback: bool,
    ) -> Result<Replacement> {
        let candidate = self.installed_package(reference).await?;
        let current = self
            .mounted
            .get(name)
            .ok_or_else(|| WireError::new(4, "目标实例尚待装载"))?;
        if candidate.name != current.package.name {
            return Err(WireError::new(3, "库替换需要相同逻辑插件名称"));
        }
        let group = current.group.clone();
        let previous = current.package.clone();
        let affected = self
            .mounted
            .iter()
            .filter(|(_, item)| item.group == group && item.package.name == candidate.name)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let mut packages = self
            .groups
            .get(&group)
            .expect("mounted group retained")
            .clone();
        for package in &mut packages {
            if package.name == candidate.name {
                *package = candidate.clone();
            }
        }
        let mut replacement = Replacement {
            group,
            candidate,
            previous,
            affected,
            packages,
            overlay: self.overlay.clone(),
            inherited: HashMap::new(),
        };
        self.prepare_replacement_states(&mut replacement, rollback)?;
        Ok(replacement)
    }

    /// DOC-11 §7：上一版本记录使用实际装载副本，回滚按各实例读取该版本的原快照。
    fn prepare_replacement_states(
        &self,
        replacement: &mut Replacement,
        rollback: bool,
    ) -> Result<()> {
        let desired = self.configuration.entries(&self.overlay);
        for name in &replacement.affected {
            let current = &desired[name];
            let mut prior = OverlayEntry::from(current);
            prior.plugin = replacement.previous.reference.clone();
            replacement.overlay.previous.insert(name.clone(), prior);
            if rollback {
                replacement.inherited.insert(
                    name.clone(),
                    self.store
                        .replacement(name, &replacement.candidate.reference)?,
                );
            }
            let mut update = OverlayEntry::from(current);
            update.plugin = replacement.candidate.reference.clone();
            replacement.overlay.instance.insert(name.clone(), update);
        }
        Ok(())
    }

    /// DOC-15 §3.2：先保存原快照和启动继承状态，再原子发布引用它们的部署叠加层。
    fn persist_replacement(
        &self,
        replacement: &Replacement,
        original: &HashMap<String, Option<payload::State>>,
    ) -> Result<()> {
        for name in &replacement.affected {
            let state = original
                .get(name)
                .ok_or_else(|| WireError::new(1, "替换事务缺少原状态记录"))?;
            self.store
                .save_replacement(name, &replacement.previous.reference, state.as_ref())?;
        }
        let inherited = if replacement.inherited.is_empty() {
            original
        } else {
            &replacement.inherited
        };
        self.store.restore_states(inherited)?;
        self.store.save_overlay(&replacement.overlay)
    }

    /// DOC-11 §7：持久化失败时用本次 SW-5 原快照恢复，恢复错误保留双方提交事实。
    async fn restore_after_persistence_failure(
        &mut self,
        replacement: Replacement,
        original: HashMap<String, Option<payload::State>>,
        problem: WireError,
    ) -> Result<()> {
        let packages = self.groups[&replacement.group].clone();
        let recovery = self
            .replace_package(
                &replacement.group,
                &replacement.previous,
                &packages,
                original.clone(),
            )
            .await;
        if recovery
            .as_ref()
            .map_or_else(replacement_committed, |()| true)
        {
            self.store.restore_states(&original)?;
            self.store.retain_replacements(&self.overlay)?;
        } else {
            self.accept_replacement(replacement)?;
        }
        match recovery {
            Ok(()) => Err(problem),
            Err(recovery) => Err(WireError::new(
                1,
                format!("保存部署失败：{problem}；恢复结果：{recovery}"),
            )),
        }
    }

    /// DOC-11 SW-9：根据监管器的提交事实更新挂载记录及后续分组启动计划。
    fn accept_replacement(&mut self, replacement: Replacement) -> Result<()> {
        for name in &replacement.affected {
            let mounted = self
                .mounted
                .get_mut(name)
                .expect("affected instance retained");
            mounted.package = replacement.candidate.clone();
            mounted.settings.plugin = replacement.candidate.reference.clone();
        }
        self.groups.insert(replacement.group, replacement.packages);
        self.overlay = replacement.overlay;
        self.logs.emit(
            2,
            "runtime",
            &format!(
                "replace plugin={} affected={}",
                replacement.candidate.name,
                replacement.affected.join(",")
            ),
        );
        self.refresh()
    }

    /// DOC-11 §7、Q0：两种插口共用监管器事务，原生代码始终由宿主执行。
    async fn replace_package(
        &self,
        group: &str,
        candidate: &Package,
        packages: &[Package],
        states: HashMap<String, Option<payload::State>>,
    ) -> Result<()> {
        match &candidate.entry {
            Entry::Native { library } => {
                self.manager
                    .restore_library(
                        group.to_owned(),
                        candidate.name.clone(),
                        library.clone(),
                        states,
                    )
                    .await?;
            }
            Entry::Process { .. } => {
                self.manager
                    .restore_group(
                        catalog::plan(group, packages, &self.project, &self.configuration.runtime)?,
                        states,
                    )
                    .await?;
            }
            Entry::Web => return Err(WireError::new(3, "网页插件需要网页宿主连接")),
        }
        Ok(())
    }
}

/// DOC-11 SW-9：提交标记来自监管器错误外层，插件自身 detail 保留在 cause 内。
fn replacement_committed(problem: &WireError) -> bool {
    problem
        .detail
        .deserialized::<payload::ReplacementFailure>()
        .is_ok_and(|failure| failure.committed)
}

/// DOC-11 §5：同一插件组件的配置更新继承原状态，组件身份变化使用自身初始状态。
fn inherited_configuration(
    before: &BTreeMap<String, InstanceSettings>,
    target: &BTreeMap<String, InstanceSettings>,
    saved: &HashMap<String, payload::State>,
) -> HashMap<String, payload::State> {
    saved
        .iter()
        .filter(|(name, _)| {
            before
                .get(*name)
                .zip(target.get(*name))
                .is_some_and(|(old, next)| {
                    old.plugin == next.plugin && old.component == next.component
                })
        })
        .map(|(name, state)| (name.clone(), state.clone()))
        .collect()
}

/// DOC-16 §5：内核错误保留说明并转换为协议通用失败。
fn kernel(problem: cordis::Error) -> WireError {
    WireError::new(1, problem.to_string())
}
