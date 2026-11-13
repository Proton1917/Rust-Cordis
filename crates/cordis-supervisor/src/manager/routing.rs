//! DOC-15 §6.1、§6.4、CP-5：服务路由、Scoped 视图和跨连接对象引用转交。
//!
//! 路由锁内只操作编号与所有权，调用和释放在锁外进行；批次发送失败归还已导出引用。

use super::*;
use generation::Binding;
use metadata::RetainedInfo;
use tokio::sync::OnceCell;

type LiveGenerations = (Option<Arc<Generation>>, Option<Arc<Generation>>);

pub(super) type Batch = cordis_routing::Batch<Route>;

pub(super) enum Target {
    Remote(RemoteObject),
    View,
    Local {
        service: Arc<dyn LocalService>,
        origin: Option<CallContext>,
    },
}
pub(super) struct Route {
    pub target: Target,
    pub info: OnceCell<Arc<RetainedInfo>>,
    pub lifetime: Option<Weak<Generation>>,
    pub view: Option<Arc<ViewRecord>>,
}
impl Route {
    pub fn remote(
        object: RemoteObject,
        info: Option<RetainedInfo>,
        lifetime: Option<Weak<Generation>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            target: Target::Remote(object),
            info: OnceCell::new_with(info.map(Arc::new)),
            lifetime,
            view: None,
        })
    }
    fn live(&self) -> Result<LiveGenerations> {
        let consumer = self
            .view
            .as_ref()
            .map(|view| {
                view.generation
                    .upgrade()
                    .ok_or_else(|| error(4, "使用方上下文纪元已退役"))
            })
            .transpose()?;
        let provider = self
            .lifetime
            .as_ref()
            .map(|lifetime| {
                lifetime
                    .upgrade()
                    .ok_or_else(|| error(4, "服务所属纪元已退役"))
            })
            .transpose()?;
        Ok((consumer, provider))
    }
}

impl Manager {
    /// DOC-16 §7、ADR-0041：转发授权使用真实连接所属的活跃上下文身份。
    pub fn request_origin(&self, connection: &Connection, context: u64) -> Result<CallContext> {
        let view = self.generation(connection.id(), context)?;
        view.active()?;
        Ok(call_origin(&view))
    }

    /// DOC-15 §11.3、ADR-0041：核心装配调用当前远端服务，参数和结果采用持久化数据。
    pub async fn invoke_service(
        &self,
        interface: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        let route = self.lookup_service_route(interface)?;
        let _live = route.live()?;
        let Target::Remote(object) = &route.target else {
            return Err(error(3, "核心装配接口需要远端提供方"));
        };
        for argument in &args {
            payload::persistent_value(argument).map_err(|problem| error(2, problem.to_string()))?;
        }
        let response = object
            .peer()
            .request(
                "invoke",
                value(&payload::Invoke {
                    object: object.id(),
                    method: method.into(),
                    args,
                    refs: Vec::new(),
                })?,
                self.0.options.invoke_timeout,
            )
            .await?;
        let result: payload::InvokeResult = decode(response.value())?;
        payload::persistent_value(&result.value)
            .map_err(|problem| error(2, problem.to_string()))?;
        Ok(result.value)
    }

    /// DOC-15 §11.3：资源登记持有原 HTTP 服务对象，清理定位同次激活的登记表。
    pub fn lookup_remote_service(&self, interface: &str) -> Result<RemoteObject> {
        let route = self.lookup_service_route(interface)?;
        let _live = route.live()?;
        match &route.target {
            Target::Remote(object) => Ok(object.clone()),
            _ => Err(error(3, "核心装配接口需要远端提供方")),
        }
    }

    fn lookup_service_route(&self, interface: &str) -> Result<Arc<Route>> {
        let requested = Interface::parse(interface)?;
        let binding = self
            .0
            .context
            .lookup_named(requested.key.as_str())
            .ok_or_else(|| error(4, "核心装配所需服务缺席"))?
            .downcast::<Binding>()
            .map_err(|_| error(2, "服务绑定需要远端路由"))?;
        if binding.interface.minor < requested.minor {
            return Err(error(3, "接口次版本不足"));
        }
        Ok(binding.route.clone())
    }

    /// ADR-0017：在监管根登记核心内置服务，实例读取时绑定真实调用者身份。
    pub fn provide_builtin(
        &self,
        interface: &str,
        methods: Vec<String>,
        service: Arc<dyn LocalService>,
    ) -> Result<()> {
        self.provide_builtin_in(&self.0.context, interface, methods, service)
            .map(|_| ())
    }

    /// DOC-15 §3.2：在指定内核视图登记内置服务，授权域由部署层创建。
    pub fn provide_builtin_in(
        &self,
        view: &Context,
        interface: &str,
        methods: Vec<String>,
        service: Arc<dyn LocalService>,
    ) -> Result<cordis::NamedProvision> {
        self.check_view(view)?;
        let interface = Interface::parse(interface)?;
        let info = payload::ObjectInfo {
            interface: interface.full.clone(),
            methods,
            local: None,
        };
        let route = Arc::new(Route {
            target: Target::Local {
                service,
                origin: None,
            },
            info: OnceCell::new_with(Some(Arc::new(RetainedInfo {
                value: info,
                _budget: None,
            }))),
            lifetime: None,
            view: None,
        });
        view.provide_named(
            interface.key.clone(),
            Arc::new(Binding {
                interface,
                route,
                generation: None,
                scoped: false,
                _view: None,
                _registration: None,
                _metadata: None,
            }),
        )
        .map_err(kernel_error)
    }

    pub(super) fn route(&self, recipient: u64, id: u64) -> Result<Arc<Route>> {
        self.0.routes.route(recipient, id)
    }

    /// DOC-16 资源协议、CP-3：按接收连接分配编号和额度，批次失败时归还。
    pub(super) fn export_route(
        &self,
        recipient: &Connection,
        route: Arc<Route>,
        batch: &mut Batch,
    ) -> Result<u64> {
        let owner = match &route.target {
            Target::Remote(object) => Some(object.peer().id()),
            _ => None,
        };
        self.0.routes.export(recipient, route, owner, batch)
    }

    pub(super) fn batch(&self, recipient: u64) -> Batch {
        self.0.routes.batch(recipient)
    }

    /// DOC-16 对象信息：首次请求取得接口和方法表，同一活跃路由复用结果。
    pub(super) async fn object_info(&self, route: &Route) -> Result<payload::ObjectInfo> {
        let _live = route.live()?;
        let info = route
            .info
            .get_or_try_init(|| async {
                let Target::Remote(object) = &route.target else {
                    return Err(error(4, "内置对象元数据缺席"));
                };
                self.0
                    .metadata
                    .remote_info(object, self.0.options.invoke_timeout)
                    .await
            })
            .await?;
        Ok(info.value.clone())
    }

    /// DOC-11 §3、DOC-15 §6.4：解析使用方服务与最低次版本，按该视图元数据建立 Scoped 对象。
    pub(super) async fn get_service(
        &self,
        generation: &ViewLease,
        interface: &str,
    ) -> Result<Arc<Route>> {
        let requested = Interface::parse(interface)?;
        let service = generation
            .context
            .get_named(requested.key.clone())
            .map_err(|problem| error(4, problem.to_string()))?;
        let binding = service
            .downcast::<Binding>()
            .map_err(|_| error(2, "服务绑定的类型需要为远端路由"))?;
        if binding.interface.minor < requested.minor {
            return Err(error(3, "接口次版本不足"));
        }
        match &binding.route.target {
            Target::Remote(object) => {
                if !object.peer().is_alive() {
                    return Err(error(4, "服务进程已结束"));
                }
                let route = if binding.scoped {
                    let meta = self.scoped_metadata(generation, &requested.key)?;
                    let cached = self.scoped_slot(generation, &binding)?;
                    cached
                        .value
                        .get_or_try_init(|| self.bind_scoped(&binding, object, &meta))
                        .await?
                        .clone()
                } else {
                    binding.route.clone()
                };
                let Target::Remote(object) = &route.target else {
                    unreachable!("remote scoped target")
                };
                Ok(Arc::new(Route {
                    target: Target::Remote(object.clone()),
                    info: OnceCell::new_with(route.info.get().cloned()),
                    lifetime: route.lifetime.clone(),
                    view: Some(generation.record.clone()),
                }))
            }
            Target::Local { service, .. } => Ok(self.local_route(generation, &binding, service)),
            Target::View => Err(error(2, "上下文租约属于资源句柄")),
        }
    }

    /// DOC-15 §3.2：内置服务绑定实际使用方身份与视图寿命，管理授权沿该身份检查。
    fn local_route(
        &self,
        generation: &ViewLease,
        binding: &Binding,
        service: &Arc<dyn LocalService>,
    ) -> Arc<Route> {
        Arc::new(Route {
            target: Target::Local {
                service: service.clone(),
                origin: Some(call_origin(generation)),
            },
            info: OnceCell::new_with(Some(
                binding.route.info.get().expect("builtin metadata").clone(),
            )),
            lifetime: Some(Arc::downgrade(&generation.generation)),
            view: Some(generation.record.clone()),
        })
    }

    /// ADR-0025：按上下文与绑定缓存视图槽，过期绑定的资源在锁外释放。
    fn scoped_slot(
        &self,
        generation: &ViewLease,
        binding: &Arc<Binding>,
    ) -> Result<Arc<views::ScopedView>> {
        let identity = Arc::as_ptr(binding) as usize;
        let (cached, discarded) = {
            let mut cache = generation.record.scoped.lock();
            let stale: Vec<_> = cache
                .iter()
                .filter_map(|(key, entry)| (entry.binding.strong_count() == 0).then_some(*key))
                .collect();
            let discarded: Vec<_> = stale.iter().filter_map(|key| cache.remove(key)).collect();
            let cached = match cache.entry(identity) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
                std::collections::hash_map::Entry::Vacant(entry) => entry
                    .insert(Arc::new(views::ScopedView {
                        binding: Arc::downgrade(binding),
                        value: OnceCell::new(),
                        _registration: generation.reserve()?,
                    }))
                    .clone(),
            };
            (cached, discarded)
        };
        drop(discarded);
        Ok(cached)
    }

    /// DOC-15 §6.4：提供方通过保留方法生成带元数据的视图，接口错误在绑定边界返回。
    async fn bind_scoped(
        &self,
        binding: &Binding,
        object: &RemoteObject,
        meta: &Value,
    ) -> Result<Arc<Route>> {
        let output = object
            .peer()
            .request(
                "invoke",
                value(&payload::Invoke {
                    object: object.id(),
                    method: "cordis.scope".into(),
                    args: vec![meta.clone()],
                    refs: Vec::new(),
                })?,
                self.0.options.invoke_timeout,
            )
            .await?;
        let result: payload::InvokeResult = decode(output.value())?;
        let reference: payload::ObjectRef = decode(&result.value)?;
        let scoped = Route::remote(
            output.import(reference.0)?,
            None,
            binding.route.lifetime.clone(),
        );
        let info = self.object_info(&scoped).await?;
        if info.interface != binding.interface.full {
            return Err(error(3, "scoped视图接口与提供方声明不匹配"));
        }
        Ok(scoped)
    }

    /// DOC-16 资源协议：移除接收方导出编号，锁外释放对象与库租约。
    pub(super) fn release(&self, recipient: u64, ids: &[u64]) -> Result<()> {
        self.0.routes.release(recipient, ids)
    }

    /// DOC-15 CP-6：连接终止后清空该连接的路由与编号计数。
    pub(super) fn reclaim_routes(&self, connection: u64) {
        self.0.routes.reclaim(connection)
    }

    /// DOC-16 资源协议：按 CBOR 资源标签转交编号，Batch 保存发送完成前的拥有引用。
    pub(super) fn transfer(
        &self,
        data: Value,
        incoming: &Payload,
        recipient: &Connection,
    ) -> Result<(Value, Batch)> {
        self.0
            .routes
            .transfer(data, incoming, recipient, imported_route)
    }

    /// DOC-15 §6.1、CP-5：检验路由寿命后锁外调用，返回值与发送后动作随响应转交。
    pub(super) async fn invoke(
        &self,
        caller: &Connection,
        request: payload::Invoke,
        incoming: &Payload,
    ) -> Result<Outgoing> {
        let route = self.route(caller.id(), request.object)?;
        let _live = route.live()?;
        match &route.target {
            Target::View => Err(error(3, "上下文租约的可调用方法为空")),
            Target::Remote(object) => {
                self.0
                    .routes
                    .invoke_remote(caller, object, request, incoming, imported_route)
                    .await
            }
            Target::Local { service, origin } => {
                let origin = origin
                    .as_ref()
                    .ok_or_else(|| error(4, "内置对象缺少调用上下文"))?;
                self.0
                    .routes
                    .invoke_local(
                        caller,
                        service.as_ref(),
                        origin,
                        request,
                        incoming,
                        imported_route,
                    )
                    .await
            }
        }
    }
}

/// ADR-0041：载荷转交只保留远端资源，纪元视图由实际服务绑定另行持有。
fn imported_route(object: RemoteObject) -> Arc<Route> {
    Route::remote(object, None, None)
}

/// ADR-0017：管理调用与转发授权共用核心已解析的上下文身份。
fn call_origin(view: &ViewLease) -> CallContext {
    CallContext {
        connection: view.component.connection.id(),
        context: view.id,
        instance: view.instance.name.clone(),
        group: view.instance.group.clone(),
        plugin: view.instance.plugin.clone(),
    }
}
