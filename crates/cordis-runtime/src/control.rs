//! DOC-15 §3.2：串行执行管理变更，并协调运行版本、原快照与部署叠加层。

mod batch;
mod contract;
mod outcomes;
mod sources;
mod web;
pub(crate) use outcomes::{BatchOutcomes, PendingBatch};
pub(crate) use sources::{SourceBindings, offline_sources};

use crate::{
    builtins::{Logs, Management, Permissions},
    catalog::{self, Catalog, Package},
    config::{self, Configuration, InstanceSettings, Overlay, OverlayEntry, Result, parameter},
    deployment::Locations,
    install,
    manifest::Entry,
    request::{Handle, Operation, Request},
    status::Diagnostics,
    storage::Store,
};
use cordis::{Context, Effect, Fiber, Realm};
use cordis_supervisor::manager::{InstanceSpec, Manager};
use cordis_up::{Value, WireError, payload};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, HashMap},
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
    manage: Option<Effect>,
}

pub struct Controller {
    web: Arc<cordis_web_deployment::Deployment>,
    pub manager: Arc<Manager>,
    pub configuration: Configuration,
    pub path: PathBuf,
    pub project: PathBuf,
    pub plugins: PathBuf,
    locations: Locations,
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
    batch_discarded: Option<cordis_types::ConfirmationDiscard>,
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
        locations: Locations,
        store: Store,
        overlay: Overlay,
        services: ControlServices,
    ) -> Result<Self> {
        let ControlServices {
            diagnostics,
            permissions,
            handle,
            logs,
        } = services;
        let project = path.parent().expect("configuration parent").to_owned();
        let plugins = locations.installed.clone();
        let outcomes = Arc::new(Mutex::new(BatchOutcomes::new(diagnostics.clone())));
        let sources = Arc::new(Mutex::new(SourceBindings::new(
            store.directory.join("sources.json"),
            plugins
                .strip_prefix(&project)
                .unwrap_or(&plugins)
                .to_owned(),
        )));
        diagnostics.lock().source_bindings = Some(sources.clone());
        let allowed = permissions.clone();
        let web = cordis_web_deployment::Deployment::new(
            manager.clone(),
            Arc::new(move |name| {
                allowed
                    .lock()
                    .get(name)
                    .is_some_and(|settings| settings.forwarder)
            }),
        )?;
        diagnostics.lock().web = Some(Arc::downgrade(&web));
        Ok(Self {
            web,
            manager,
            configuration,
            path,
            project,
            plugins,
            locations,
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
            batch_discarded: None,
            sources,
        })
    }

    /// DOC-15 §3.2：发现来源、建立进程分组，再按检查点挂载实例；局部错误记入诊断。
    pub async fn start(&mut self) -> Result<()> {
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
                    if matches!(package.entry, Entry::Web) {
                        continue;
                    }
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
                &self.locations,
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
        if let Err(problem) = self.sources.lock().confirm(&self.manager.snapshot()) {
            self.logs
                .emit(0, "runtime", &format!("代码来源保存失败：{problem}"));
        }
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
                self.replace_batch(&[(name.clone(), plugin.clone())], None)
                    .await?;
            }
            Operation::ReplaceBatch {
                replacements,
                configuration,
                ..
            } => {
                self.replace_batch(replacements, configuration.clone()?)
                    .await?;
            }
            Operation::Rollback(name) => self.rollback(name).await?,
            Operation::Reload(configuration) => {
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
            &self.configuration.runtime,
            &self.locations,
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
        let mut catalog = Catalog::default();
        let mut errors = catalog
            .discover(
                &self.locations.sources,
                &self.configuration.runtime,
                &self.locations,
                &self.catalog,
            )
            .await?;
        if self.locations.sources != self.plugins {
            errors.extend(
                catalog
                    .discover(
                        &self.plugins,
                        &self.configuration.runtime,
                        &self.locations,
                        &self.catalog,
                    )
                    .await?,
            );
        }
        for source_dir in &self.configuration.runtime.source_dirs {
            let directory = config::resolve_path(&self.project, source_dir);
            match catalog
                .discover(
                    &directory,
                    &self.configuration.runtime,
                    &self.locations,
                    &self.catalog,
                )
                .await
            {
                Ok(found) => errors.extend(found),
                Err(problem) => {
                    errors.insert(directory.display().to_string(), problem.to_string());
                }
            }
        }
        self.catalog = catalog;
        self.installed.clear();
        let mut diagnostics = self.diagnostics.lock();
        diagnostics.sources = self.catalog.packages.values().cloned().collect();
        diagnostics.plugin_errors = errors;
        Ok(())
    }

    /// DOC-15 §3.2、DOC-11 §7：装载前固定来源副本，使上一版本引用持续指向原字节。
    async fn installed_package(&mut self, reference: &str) -> Result<Package> {
        if let Some(package) = self.installed.get(reference) {
            return Ok(package.clone());
        }
        let source = self.catalog.get(reference)?;
        if self.locations.app
            && (source.source.starts_with(&self.locations.sources)
                || source.reference.starts_with("installed-"))
        {
            self.installed.insert(reference.into(), source.clone());
            return Ok(source);
        }
        let installed =
            install::install(&source.source, &self.project, &self.project, &self.plugins)?;
        let package = if installed == source.reference {
            source
        } else {
            catalog::inspect(
                installed.clone(),
                self.plugins.join(&installed),
                &self.configuration.runtime,
                &self.locations,
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
        let plan = catalog::plan(
            group,
            &packages,
            &self.project,
            &self.configuration.runtime,
            &self.locations,
        )?;
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
        if matches!(package.entry, Entry::Web) {
            return self.mount_web(name, settings, package);
        }
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
                    component: component.clone(),
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
        if package.web.is_some() {
            self.web
                .mount(Self::web_spec(name, &settings, &package, &view)?)?;
        }
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
            .isolate_named(config::MANAGE, Realm::new())
            .isolate_named("cordis.forward@1", Realm::new()))
    }

    /// DOC-15 §3.2：按配置授权提供实例专有管理对象，调用时继续逐方法鉴权。
    fn provide_management(
        &self,
        name: &str,
        settings: &InstanceSettings,
        view: &Context,
    ) -> Result<Option<Effect>> {
        let mut provisions = Vec::new();
        if settings.forwarder {
            provisions.push(self.web.forwarder(view)?);
        }
        if !settings
            .grants
            .get(config::MANAGE)
            .is_none_or(Vec::is_empty)
        {
            provisions.push(
                self.manager.provide_builtin_in(
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
                )?,
            );
        }
        if provisions.is_empty() {
            return Ok(None);
        }
        view.effect("instance-permissions", move |scope| {
            scope.defer_async(move || async move {
                for provision in provisions {
                    provision.dispose().await?;
                }
                Ok(())
            })
        })
        .map(Some)
        .map_err(kernel)
    }

    /// T-EFF-2、CP-7：释放实例与授权，最后一个成员退出后释放其分组进程。
    async fn remove(&mut self, name: &str) -> Result<()> {
        let Some(mounted) = self.mounted.remove(name) else {
            return Ok(());
        };
        if matches!(mounted.package.entry, Entry::Web) {
            self.web.remove(name).await?;
            return Ok(());
        }
        self.web.remove(name).await?;
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

    /// CP-4：实例稳定等待受 apply 截止时间限制，保留内核返回的加载错误。
    async fn settle(&self, name: &str) -> Result<()> {
        let mounted = self.mounted.get(name).expect("mounted instance retained");
        tokio::time::timeout(
            Duration::from_millis(self.configuration.runtime.lifecycle_timeout_ms),
            mounted.fiber.settled(),
        )
        .await
        .map_err(|_| WireError::new(6, "实例加载截止时间到达"))?
        .map_err(kernel)
    }

    /// DOC-15 §3.2、§11.4：配置与插件批次共用状态保存、页面协调及持久提交。
    async fn commit_configuration(
        &mut self,
        configuration: Configuration,
        overlay: Overlay,
    ) -> Result<()> {
        self.apply_deployment_batch(&[], Some(configuration), overlay)
            .await
    }

    /// ADR-0041/0052：部署实例按运行时目录恢复，页面快照由网页事务持有。
    fn deployment_states(
        &self,
        mut states: HashMap<String, Option<payload::State>>,
    ) -> HashMap<String, Option<payload::State>> {
        states.retain(|name, _| self.mounted.contains_key(name));
        states
    }

    /// DOC-11 §7：回滚是把实例换回上一版本引用的单项批次，状态继承当前导出值。
    async fn rollback(&mut self, name: &str) -> Result<()> {
        let previous = self
            .overlay
            .previous
            .get(name)
            .cloned()
            .ok_or_else(|| WireError::new(4, "实例的上一版本引用缺席"))?;
        self.replace_batch(&[(name.to_owned(), previous.plugin)], None)
            .await
    }
}

/// DOC-11 SW-9：提交标记来自监管器错误外层，插件自身 detail 保留在 cause 内。
fn replacement_committed(problem: &WireError) -> bool {
    problem
        .detail
        .deserialized::<payload::ReplacementFailure>()
        .is_ok_and(|failure| failure.committed)
}

/// DOC-16 §5：内核错误保留说明并转换为协议通用失败。
fn kernel(problem: cordis::Error) -> WireError {
    WireError::new(1, problem.to_string())
}
