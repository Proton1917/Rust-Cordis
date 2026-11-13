//! DOC-15 §6.4、ADR-0025：派生上下文、命名隔离域、Scoped 缓存与子组件的寿命。
//!
//! 视图归属原加载纪元，最后一个租约释放时撤回编号与元数据，清理回调保持原视图。

use super::*;
use generation::{Binding, Configuration, Registration};
use routing::Target;
use tokio::sync::OnceCell;

pub(super) type RealmKey = (Option<u64>, String);

pub(super) struct RealmLease {
    name: RealmKey,
    realm: cordis::Realm,
    core: Weak<Core>,
}

struct PageNamespace(u64);

impl Drop for RealmLease {
    fn drop(&mut self) {
        if let Some(core) = self.core.upgrade() {
            let mut realms = core.realms.lock();
            if realms
                .get(&self.name)
                .is_some_and(|current| std::ptr::eq(current.as_ptr(), self))
            {
                realms.remove(&self.name);
            }
        }
    }
}

pub(super) struct ScopedView {
    pub binding: Weak<Binding>,
    pub value: OnceCell<Arc<Route>>,
    pub _registration: Arc<Registration>,
}

pub(super) struct ViewRecord {
    pub id: u64,
    pub generation: Weak<Generation>,
    pub context: Mutex<Option<Context>>,
    pub scoped: Mutex<HashMap<usize, Arc<ScopedView>>>,
    core: Weak<Core>,
    connection: Connection,
    retained: Mutex<Option<(Arc<Registration>, Arc<MetadataLease>)>>,
}

impl ViewRecord {
    /// DOC-15 §6.4：登记同一加载纪元的视图及额度租约，由退役路径统一释放。
    pub fn new(
        id: u64,
        context: Context,
        generation: Weak<Generation>,
        manager: &Manager,
        connection: Connection,
        retained: Option<(Arc<Registration>, Arc<MetadataLease>)>,
    ) -> Self {
        Self {
            id,
            generation,
            context: Mutex::new(Some(context)),
            scoped: Mutex::new(HashMap::new()),
            core: Arc::downgrade(&manager.0),
            connection,
            retained: Mutex::new(retained),
        }
    }

    /// DOC-15 CP-6、§6.4：撤回视图和缓存后通知宿主；断连时依连接整表回收。
    pub fn retire(&self) {
        let context = self.context.lock().take();
        if context.is_none() {
            return;
        }
        let scoped = std::mem::take(&mut *self.scoped.lock());
        let retained = self.retained.lock().take();
        if let Some(core) = self.core.upgrade() {
            core.contexts.lock().remove(&self.id);
        }
        drop((context, scoped, retained));
        if self.connection.is_alive() {
            let notification = value(&payload::Context { context: self.id });
            if let Err(problem) = notification
                .and_then(|params| self.connection.try_notify("release_context", params))
                && let Some(core) = self.core.upgrade()
            {
                core.context
                    .logger()
                    .warn(saved_error(&problem).to_string());
            }
        }
    }
}

impl Drop for ViewRecord {
    fn drop(&mut self) {
        self.retire();
        if let Some(generation) = self.generation.upgrade() {
            generation.handles.lock().remove(&self.id);
        }
    }
}

impl Manager {
    /// DOC-15 §11.2、ADR-0041：页面身份存于核心上下文，派生视图和子组件沿祖先链继承。
    pub fn page_context(&self, parent: &Context, connection: u64) -> Context {
        parent.extend(PageNamespace(connection))
    }

    /// ADR-0025：部署配置与远端请求共享命名隔离域及逐字段元数据覆盖规则。
    pub fn derive_context(
        &self,
        parent: &Context,
        options: &payload::ContextOptions,
    ) -> Result<Context> {
        self.running()?;
        let mut context = parent.clone();
        for isolation in &options.isolate {
            let key = Interface::key(&isolation.interface)?;
            let realm = match &isolation.realm {
                Some(name) => {
                    let lease = self.realm_lease(&context, name);
                    let realm = lease.realm;
                    context = context.extend(lease);
                    realm
                }
                None => cordis::Realm::new(),
            };
            context = context.isolate_named(key, realm);
        }
        for interception in &options.intercept {
            self.check_interception(&context, &interception.meta)?;
            let key = Interface::key(&interception.interface)?;
            payload::persistent_value(&interception.meta)
                .map_err(|problem| error(2, problem.to_string()))?;
            let inherited = context
                .named_meta(&key)
                .and_then(|meta| meta.downcast::<Value>().ok());
            let meta = match (inherited.as_deref(), &interception.meta) {
                (Some(Value::Map(previous)), Value::Map(next)) => {
                    let mut combined = previous.clone();
                    for (key, value) in next {
                        if let Some((_, old)) = combined.iter_mut().find(|(old, _)| old == key) {
                            *old = value.clone();
                        } else {
                            combined.push((key.clone(), value.clone()));
                        }
                    }
                    Value::Map(combined)
                }
                _ => interception.meta.clone(),
            };
            context = context.intercept_named(key, Arc::new(meta));
        }
        Ok(context)
    }

    /// ADR-0025：同名域共享一份身份，最后一个视图释放时由 RealmLease 撤回登记。
    fn realm_lease(&self, context: &Context, name: &str) -> Arc<RealmLease> {
        let namespace = context
            .extension::<PageNamespace>()
            .map(|namespace| namespace.0);
        let name = (namespace, name.to_owned());
        let mut realms = self.0.realms.lock();
        match realms.get(&name).and_then(Weak::upgrade) {
            Some(lease) => lease,
            None => {
                let lease = Arc::new(RealmLease {
                    name: name.clone(),
                    realm: cordis::Realm::new(),
                    core: Arc::downgrade(&self.0),
                });
                realms.insert(name, Arc::downgrade(&lease));
                lease
            }
        }
    }

    /// DOC-15 §6.4：有效父视图派生带额度的远端租约，响应入队后转交资源引用。
    pub(super) fn derive_remote(
        &self,
        connection: &Connection,
        request: payload::ContextDerive,
    ) -> Result<Outgoing> {
        let parent = self.generation(connection.id(), request.context)?;
        parent.active()?;
        let registration = parent.reserve()?;
        let options = payload::ContextOptions {
            isolate: request.isolate,
            intercept: request.intercept,
        };
        let context = self.derive_context(&parent.context, &options)?;
        let bytes = self.view_metadata_bytes(&context, &options)?;
        let metadata = self.retain_metadata(connection, bytes)?;
        let id = self.id();
        let view = Arc::new(ViewRecord::new(
            id,
            context,
            Arc::downgrade(&parent.generation),
            self,
            connection.clone(),
            Some((registration, metadata)),
        ));
        parent.handles.lock().insert(id, Arc::downgrade(&view));
        self.0.contexts.lock().insert(id, Arc::downgrade(&view));
        self.export_view(connection, &parent, view)
    }

    /// DOC-15 CP-3、§6.4：计量派生后的实际元数据，容量错误在登记边界返回。
    fn view_metadata_bytes(
        &self,
        context: &Context,
        options: &payload::ContextOptions,
    ) -> Result<usize> {
        let mut bytes = metadata::strings(options.isolate.iter().flat_map(|isolation| {
            std::iter::once(isolation.interface.as_str()).chain(isolation.realm.as_deref())
        }))?;
        for interception in &options.intercept {
            let key = Interface::key(&interception.interface)?;
            let meta = context
                .named_meta(&key)
                .and_then(|meta| meta.downcast::<Value>().ok())
                .expect("derived interception value");
            let size = metadata::value_bytes(&meta)?;
            bytes = bytes
                .checked_add(key.len())
                .and_then(|total| total.checked_add(size))
                .ok_or_else(|| error(8, "派生视图元数据超出地址空间"))?;
        }
        Ok(bytes)
    }

    /// DOC-16 资源协议：视图通过对象释放通道持有，发送失败由 Batch 回收。
    fn export_view(
        &self,
        connection: &Connection,
        parent: &ViewLease,
        view: Arc<ViewRecord>,
    ) -> Result<Outgoing> {
        let id = view.id;
        let info = self.retain_info(
            connection,
            payload::ObjectInfo {
                interface: "cordis.context-view@1.0".into(),
                methods: Vec::new(),
                local: None,
            },
        )?;
        let route = Arc::new(Route {
            target: Target::View,
            info: OnceCell::new_with(Some(Arc::new(info))),
            lifetime: Some(Arc::downgrade(&parent.generation)),
            view: Some(view),
        });
        let mut batch = self.batch(connection.id());
        let object = self.export_route(connection, route, &mut batch)?;
        let result = value(&payload::ContextDerived {
            context: id,
            object,
            refs: batch.ids.clone(),
        })?;
        Ok(batch.outgoing(result))
    }

    /// DOC-15 §6.4：装入调用方自身组件，子实例和登记租约随父视图的效应清理。
    pub(super) async fn mount_child(
        &self,
        parent: ViewLease,
        name: String,
        config: Value,
    ) -> Result<u64> {
        parent.active()?;
        let registration = parent.reserve()?;
        let source = &parent.component;
        let plugin = source
            .source
            .plugins
            .iter()
            .find(|plugin| plugin.name == source.plugin && plugin.library == source.library)
            .ok_or_else(|| error(2, "调用方插件描述符已失效"))?;
        let info = plugin
            .components
            .iter()
            .find(|component| component.name == name)
            .ok_or_else(|| error(2, "调用方插件描述符中缺少目标组件"))?;
        let component = self.component_from_description(
            &source.group,
            &source.connection,
            plugin,
            info,
            &source.source,
            &source._source_metadata,
        )?;
        self.resolve(&component, &config).await?;
        parent.active()?;
        let id = self.id();
        let data = Arc::new(InstanceData {
            name: format!("{}/{}:{id}", parent.instance.name, name),
            group: source.group.clone(),
            plugin: source.plugin.clone(),
            component: name,
            current: Mutex::new(Weak::new()),
            config: Mutex::new(config.clone()),
            inherited: Mutex::new(None),
            error: Mutex::new(None),
        });
        let definition = generation::definition(Arc::downgrade(&self.0), component);
        let child = parent
            .context
            .mount_with(
                &definition,
                Configuration {
                    instance: data,
                    config,
                },
            )
            .map_err(kernel_error)?;
        let retained = parent.clone();
        parent
            .context
            .effect("remote-child", move |scope| {
                scope.defer(move || {
                    drop((retained, registration));
                    Ok(())
                })
            })
            .map_err(kernel_error)?;
        child.settled().await.map_err(kernel_error)?;
        Ok(id)
    }
}
