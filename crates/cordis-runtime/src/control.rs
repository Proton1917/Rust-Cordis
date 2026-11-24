//! DOC-15 §3.2：串行执行管理变更，并协调运行版本、原快照与部署叠加层。

mod batch;
mod contract;
mod outcomes;
mod permissions;
mod sources;
mod web;
pub(crate) use outcomes::{BatchOutcomes, PendingBatch};
use sources::SourceBindings;
pub(crate) use sources::offline_sources;

use crate::{
    builtins::{Logs, Management},
    catalog::{self, Catalog, Package},
    config::{self, Configuration, InstanceSettings, Overlay, OverlayEntry, Result, parameter},
    deployment::Locations,
    install,
    manifest::Entry,
    request::{Handle, Operation, Request},
    status::Diagnostics,
    storage::Store,
};
use cordis::{Context, Effect, Realm};
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
    scope: Arc<MountScope>,
}

/// A mounted declaration selects one actual lifecycle owner. Resolve the current
/// fiber through that owner so a generation change never leaves a cached old fiber.
struct InstanceOwners<'a> {
    mounted: &'a BTreeMap<String, Mounted>,
    manager: &'a Manager,
    web: &'a cordis_web_deployment::Deployment,
}

impl InstanceOwners<'_> {
    fn fiber(&self, name: &str) -> Result<cordis::Fiber> {
        let mounted = self
            .mounted
            .get(name)
            .ok_or_else(|| WireError::new(4, format!("部署实例 {name} 缺席")))?;
        match mounted.package.entry {
            Entry::Web => self.web.instance_fiber(name),
            _ => self.manager.instance_fiber(name),
        }
    }
}

use permissions::MountScope;

pub struct Controller {
    web: Arc<cordis_web_deployment::Deployment>,
    pub manager: Arc<Manager>,
    pub configuration: Configuration,
    runtime: config::RuntimeSettings,
    pub path: PathBuf,
    pub project: PathBuf,
    pub plugins: PathBuf,
    locations: Locations,
    pub store: Store,
    pub overlay: Overlay,
    pub diagnostics: Arc<Mutex<Diagnostics>>,
    pub handle: Handle,
    pub logs: Logs,
    catalog: Catalog,
    mounted: BTreeMap<String, Mounted>,
    groups: BTreeMap<String, Vec<Package>>,
    installed: HashMap<String, Package>,
    outcomes: Arc<Mutex<BatchOutcomes>>,
    pub(crate) batch_restored: Option<Option<cordis_types::ConfirmationDiscard>>,
    sources: SourceBindings,
    pub(crate) recovery_required: Option<WireError>,
    pub(crate) last_cleanup_error: Option<WireError>,
    held_target: Option<batch::PendingTarget>,
}

/// DOC-15 §3.2：管理器与运行循环共享的诊断、授权、队列和日志入口。
pub struct ControlServices {
    pub diagnostics: Arc<Mutex<Diagnostics>>,
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
            handle,
            logs,
        } = services;
        let project = path.parent().expect("configuration parent").to_owned();
        let plugins = locations.installed.clone();
        let outcomes = diagnostics.lock().outcomes.clone();
        let sources = SourceBindings::new(
            store.directory.join("sources.json"),
            plugins
                .strip_prefix(&project)
                .unwrap_or(&plugins)
                .to_owned(),
        );
        let allowed = diagnostics.clone();
        let web = cordis_web_deployment::Deployment::new(
            manager.clone(),
            Arc::new(move |name| {
                let deployment = allowed.lock();
                deployment.candidate_forwarders.as_ref().map_or_else(
                    || {
                        deployment
                            .desired
                            .get(name)
                            .is_some_and(|settings| settings.forwarder)
                    },
                    |names| names.contains(name),
                )
            }),
        )?;
        diagnostics.lock().web = Some(Arc::downgrade(&web));
        Ok(Self {
            web,
            manager,
            runtime: configuration.runtime.clone(),
            configuration,
            path,
            project,
            plugins,
            locations,
            store,
            overlay,
            diagnostics,
            handle,
            logs,
            catalog: Catalog::default(),
            mounted: BTreeMap::new(),
            groups: BTreeMap::new(),
            installed: HashMap::new(),
            outcomes,
            batch_restored: None,
            sources,
            recovery_required: None,
            last_cleanup_error: None,
            held_target: None,
        })
    }

    /// DOC-15 §3.2：发现来源、建立进程分组，再按检查点挂载实例；局部错误记入诊断。
    pub async fn start(&mut self, candidate: Option<Result<Configuration>>) -> Result<()> {
        self.sources.load()?;
        self.discover().await?;
        self.start_target().await?;
        if let Some(candidate) = candidate
            && let Err(problem) = self.reload_configuration(candidate).await
        {
            self.logs
                .emit(0, "runtime", &format!("启动配置应用失败：{problem}"));
        }
        Ok(())
    }

    /// DOC-15 §3.2：已授权请求由单一队列按方法分派，具体事务负责持久化。
    pub async fn execute(&mut self, request: &Request) -> Result<Value> {
        self.last_cleanup_error = None;
        self.batch_restored = None;
        if let Some(error) = &self.recovery_required
            && !matches!(request.operation, Operation::Shutdown)
        {
            return Err(error.clone());
        }
        if let Err(problem) = self.confirm_sources() {
            self.logs
                .emit(0, "runtime", &format!("代码来源保存失败：{problem}"));
        }
        self.execute_operation(request).await
    }

    pub(crate) fn confirm_sources(&mut self) -> Result<()> {
        if let Some(problem) = &self.recovery_required {
            return Err(problem.clone());
        }
        self.sources.confirm(&InstanceOwners {
            mounted: &self.mounted,
            manager: &self.manager,
            web: &self.web,
        })
    }

    pub(crate) fn instance_fiber(&self, name: &str) -> Result<cordis::Fiber> {
        InstanceOwners {
            mounted: &self.mounted,
            manager: &self.manager,
            web: &self.web,
        }
        .fiber(name)
        .map_err(|problem| {
            self.diagnostics
                .lock()
                .instance_errors
                .get(name)
                .map(|message| WireError::new(problem.code, message.clone()))
                .unwrap_or(problem)
        })
    }

    /// Failure details come from the same owners used by the public status view.
    /// The full snapshots are needed only on a failed contract check.
    pub(crate) fn instance_failure(&self, name: &str) -> Option<String> {
        self.diagnostics
            .lock()
            .instance_errors
            .get(name)
            .cloned()
            .or_else(|| {
                self.manager
                    .snapshot()
                    .instances
                    .into_iter()
                    .chain(self.web.snapshot())
                    .find(|instance| instance.name == name)
                    .and_then(|instance| instance.error)
                    .map(|error| error.message)
            })
    }

    fn record_decision(
        &mut self,
        result: std::result::Result<crate::storage::Committed, crate::storage::CommitError>,
    ) -> Result<()> {
        use crate::storage::CommitError;
        match result {
            Ok(decision) => {
                if let Some(problem) = decision.cleanup_error {
                    self.report_cleanup("暂存目录", problem);
                }
                Ok(())
            }
            Err(CommitError::NotCommitted(problem)) => Err(problem),
            Err(CommitError::RecoveryRequired(problem)) => {
                let error = WireError::new(1, format!("部署决定需要从磁盘恢复：{problem}"));
                self.require_recovery(error)
            }
        }
    }

    fn report_cleanup(&mut self, resource: &str, problem: WireError) {
        self.logs.emit(
            1,
            "runtime",
            &format!("部署已提交，{resource}清理失败：{problem}"),
        );
        self.last_cleanup_error.get_or_insert(problem);
    }

    fn require_recovery(&mut self, problem: WireError) -> Result<()> {
        {
            let mut diagnostics = self.diagnostics.lock();
            diagnostics.phase = "recovery-required".into();
            diagnostics.recovery_error = Some(problem.to_string());
        }
        self.recovery_required = Some(problem.clone());
        Err(problem)
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
                self.reload_configuration(configuration.clone()).await?;
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
            &self.runtime,
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
        if self.configuration.contains_entry(&self.overlay, name) {
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
            .entry(&self.overlay, name)
            .ok_or_else(|| WireError::new(4, format!("实例 {name} 缺席")))
    }

    /// DOC-15 §3.2：重读插件目录并刷新来源诊断，已有安装副本继续可用。
    async fn discover(&mut self) -> Result<()> {
        let mut catalog = Catalog::default();
        let mut errors = catalog
            .discover(
                &self.locations.sources,
                &self.runtime,
                &self.locations,
                &self.catalog,
            )
            .await?;
        if self.locations.sources != self.plugins {
            errors.extend(
                catalog
                    .discover(&self.plugins, &self.runtime, &self.locations, &self.catalog)
                    .await?,
            );
        }
        for source_dir in &self.runtime.source_dirs {
            let directory = config::resolve_path(&self.project, source_dir);
            match catalog
                .discover(&directory, &self.runtime, &self.locations, &self.catalog)
                .await
            {
                Ok(found) => errors.extend(found),
                Err(problem) => {
                    errors.insert(directory.display().to_string(), problem.to_string());
                }
            }
        }
        self.installed.retain(|reference, package| {
            reference == &package.reference
                && reference.starts_with("installed-")
                && catalog
                    .packages
                    .get(reference)
                    .is_some_and(|current| current.source == package.source)
        });
        self.catalog = catalog;
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
                &self.runtime,
                &self.locations,
            )
            .await?
        };
        self.catalog.packages.insert(installed, package.clone());
        self.installed.insert(reference.into(), package.clone());
        Ok(package)
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
    fn refresh(&mut self) -> Result<()> {
        self.sources.track(
            &self.configuration,
            &self.overlay,
            self.mounted
                .iter()
                .map(|(name, mounted)| (name, &mounted.settings, &mounted.package)),
        );
        let snapshot = self.manager.snapshot();
        let descriptors: HashMap<_, _> = snapshot
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .plugins
                    .iter()
                    .map(move |plugin| ((group.name.as_str(), plugin.name.as_str()), plugin))
            })
            .collect();
        for mounted in self.mounted.values() {
            if let Some(descriptor) =
                descriptors.get(&(mounted.group.as_str(), mounted.package.name.as_str()))
                && let Some(package) = self.catalog.packages.get_mut(&mounted.package.reference)
            {
                package.descriptor = Some((*descriptor).clone());
            }
        }
        let mut diagnostics = self.diagnostics.lock();
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

    fn group_plan(
        &self,
        name: &str,
        packages: &[Package],
    ) -> Result<cordis_supervisor::manager::GroupSpec> {
        catalog::plan(
            name,
            packages,
            &self.project,
            &self.runtime,
            &self.locations,
        )
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
            .isolate_named(config::MANAGE_V2, Realm::new())
            .isolate_named("cordis.forward@1", Realm::new()))
    }

    /// DOC-15 §3.2：按配置授权提供实例专有管理对象，调用时继续逐方法鉴权。
    fn provide_management(
        &self,
        settings: &InstanceSettings,
        view: &Context,
    ) -> Result<Option<Effect>> {
        if !settings.forwarder
            && settings.management_grants(true).is_empty()
            && settings.management_grants(false).is_empty()
        {
            return Ok(None);
        }
        let mut registration_error = None;
        let effect = view.effect("instance-permissions", |scope| {
            let register = || -> Result<()> {
                if settings.forwarder {
                    scope
                        .adopt(self.web.forwarder(view)?.effect())
                        .map_err(kernel)?;
                }
                for (interface, legacy, methods) in [
                    (
                        "cordis.manage@1.1",
                        true,
                        &["install", "replace_batch", "shutdown"][..],
                    ),
                    ("cordis.manage@2.0", false, config::METHODS),
                ] {
                    let grants = settings.management_grants(legacy);
                    if grants.is_empty() {
                        continue;
                    }
                    let mut methods: Vec<String> =
                        methods.iter().map(|method| (*method).into()).collect();
                    if !legacy {
                        methods.push("result".into());
                    }
                    let provision = self.manager.provide_builtin_in(
                        view,
                        interface,
                        methods,
                        Arc::new(Management {
                            legacy,
                            grants: grants.to_vec(),
                            control: self.handle.clone(),
                            configuration: self.path.clone(),
                            outcomes: self.outcomes.clone(),
                            logs: self.logs.clone(),
                        }),
                    )?;
                    scope.adopt(provision.effect()).map_err(kernel)?;
                }
                Ok(())
            };
            register().map_err(|problem| {
                let error = cordis::Error::new(problem.message.clone());
                registration_error = Some(problem);
                error
            })
        });
        effect
            .map(Some)
            .map_err(|problem| registration_error.unwrap_or_else(|| kernel(problem)))
    }

    /// DOC-15 §3.2、§11.4：配置与插件批次共用状态保存、页面协调及持久提交。
    async fn commit_configuration(
        &mut self,
        configuration: Configuration,
        overlay: Overlay,
    ) -> Result<()> {
        self.apply_deployment_batch(&[], Some(configuration), overlay, true)
            .await
    }

    async fn reload_configuration(&mut self, configuration: Result<Configuration>) -> Result<()> {
        let result = match configuration {
            Ok(configuration) => {
                self.commit_configuration(configuration, self.overlay.clone())
                    .await
            }
            Err(problem) => Err(problem),
        };
        self.diagnostics.lock().configuration_error =
            result.as_ref().err().map(ToString::to_string);
        result
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

/// DOC-16 §5：内核错误保留说明并转换为协议通用失败。
fn kernel(problem: cordis::Error) -> WireError {
    WireError::new(1, problem.to_string())
}
