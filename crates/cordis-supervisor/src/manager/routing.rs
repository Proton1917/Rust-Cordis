//! DOC-15 §6.1、§6.4、CP-5：服务路由、Scoped 视图和跨连接对象引用转交。
//!
//! 路由锁内只操作编号与所有权，调用和释放在锁外进行；批次发送失败归还已导出引用。

use super::*;
use generation::Binding;
use metadata::RetainedInfo;
use tokio::sync::OnceCell;

type LiveGenerations = (Option<Arc<Generation>>, Option<Arc<Generation>>);

#[derive(Clone, Debug)]
pub struct CallContext {
    pub connection: u64,
    pub context: u64,
    pub instance: String,
    pub group: String,
    pub plugin: String,
}

pub struct Arguments {
    pub values: Vec<Value>,
    objects: HashMap<u64, RemoteObject>,
}
impl Arguments {
    pub fn import(&self, id: u64) -> Result<RemoteObject> {
        self.objects
            .get(&id)
            .cloned()
            .ok_or_else(|| error(4, "参数资源编号缺席"))
    }
}

pub trait LocalService: Send + Sync + 'static {
    fn invoke<'a>(
        &'a self,
        context: &'a CallContext,
        method: &'a str,
        args: &'a Arguments,
    ) -> HandlerFuture<'a, Result<LocalReply>>;
}

pub struct LocalReply {
    pub value: Value,
    pub after_write: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl From<Value> for LocalReply {
    fn from(value: Value) -> Self {
        Self {
            value,
            after_write: None,
        }
    }
}

impl LocalReply {
    pub fn new(value: Value) -> Self {
        value.into()
    }

    pub fn with_after_write(value: Value, action: impl FnOnce() + Send + 'static) -> Self {
        Self {
            value,
            after_write: Some(Box::new(action)),
        }
    }
}

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
pub(super) struct Batch {
    core: Weak<Core>,
    recipient: u64,
    pub ids: Vec<u64>,
    committed: bool,
}

impl Drop for Batch {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some(core) = self.core.upgrade() {
            let discarded = {
                let mut routes = core.routes.lock();
                let discarded = self
                    .ids
                    .iter()
                    .filter_map(|id| routes.entries.remove(&(self.recipient, *id)))
                    .collect::<Vec<_>>();
                if let Some(counter) = routes.counters.get_mut(&self.recipient) {
                    counter.active -= discarded.len();
                }
                discarded
            };
            drop(discarded);
        }
    }
}

impl Batch {
    fn commit(mut self) {
        self.committed = true;
    }
    pub fn outgoing(self, value: Value) -> Outgoing {
        Outgoing::with_commit(value, move || self.commit())
    }
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
        let routes = self.0.routes.lock();
        routes
            .entries
            .get(&(recipient, id))
            .cloned()
            .ok_or_else(|| error(4, "对象编号已退役或属于另一条连接"))
    }

    /// DOC-16 资源协议、CP-3：按接收连接分配编号和额度，批次失败时归还。
    pub(super) fn export_route(
        &self,
        recipient: &Connection,
        route: Arc<Route>,
        batch: &mut Batch,
    ) -> Result<u64> {
        let mut routes = self.0.routes.lock();
        if !recipient.is_alive() {
            return Err(error(4, "接收连接已结束"));
        }
        let counter = routes
            .counters
            .entry(recipient.id())
            .or_insert(ExportCounter { next: 1, active: 0 });
        if counter.active >= self.0.options.max_exports {
            return Err(error(8, "该连接的活跃出口资源已达上限"));
        }
        let id = counter.next;
        counter.next = counter
            .next
            .checked_add(1)
            .ok_or_else(|| error(8, "出口编号空间已耗尽"))?;
        counter.active += 1;
        routes.entries.insert((recipient.id(), id), route);
        batch.ids.push(id);
        Ok(id)
    }

    pub(super) fn batch(&self, recipient: u64) -> Batch {
        Batch {
            core: Arc::downgrade(&self.0),
            recipient,
            ids: Vec::new(),
            committed: false,
        }
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
                let response = self
                    .lifecycle_request(
                        object.peer(),
                        "object_info",
                        value(&payload::ObjectInfoRequest {
                            object: object.id(),
                        })?,
                        self.0.options.invoke_timeout,
                    )
                    .await?;
                let mut info: payload::ObjectInfo = decode(response.value())?;
                Interface::parse(&info.interface)?;
                info.local = Some(payload::LocalObject {
                    process: object.peer().id(),
                    object: object.id(),
                });
                self.retain_info(object.peer(), info).map(Arc::new)
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
                let meta = generation
                    .context
                    .named_meta(&requested.key)
                    .and_then(|meta| meta.downcast::<Value>().ok());
                let meta = meta.filter(|meta| match meta.as_ref() {
                    Value::Null => false,
                    Value::Map(fields) => !fields.is_empty(),
                    _ => true,
                });
                let route = match meta.filter(|_| binding.scoped) {
                    Some(meta) => {
                        let cached = self.scoped_slot(generation, &binding)?;
                        cached
                            .value
                            .get_or_try_init(|| self.bind_scoped(&binding, object, &meta))
                            .await?
                            .clone()
                    }
                    None => binding.route.clone(),
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
                origin: Some(CallContext {
                    connection: generation.component.connection.id(),
                    context: generation.id,
                    instance: generation.instance.name.clone(),
                    group: generation.instance.group.clone(),
                    plugin: generation.instance.plugin.clone(),
                }),
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
        let discarded = {
            let mut routes = self.0.routes.lock();
            let next = routes
                .counters
                .get(&recipient)
                .map_or(1, |counter| counter.next);
            for id in ids {
                if *id == 0 || *id >= next {
                    return Err(error(4, "release编号属于该连接已分配范围之外"));
                }
            }
            let discarded = ids
                .iter()
                .filter_map(|id| routes.entries.remove(&(recipient, *id)))
                .collect::<Vec<_>>();
            if let Some(counter) = routes.counters.get_mut(&recipient) {
                counter.active -= discarded.len();
            }
            discarded
        };
        drop(discarded);
        Ok(())
    }

    /// DOC-15 CP-6：连接终止后清空该连接的路由与编号计数。
    pub(super) fn reclaim_routes(&self, connection: u64) {
        let discarded = {
            let mut routes = self.0.routes.lock();
            let ids: Vec<_> = routes.entries.iter().filter_map(|(id, route)| {
                let owner_ended = matches!(&route.target, Target::Remote(object) if object.peer().id() == connection);
                (id.0 == connection || owner_ended).then_some(*id)
            }).collect();
            let discarded: Vec<_> = ids
                .iter()
                .filter_map(|id| routes.entries.remove(id).map(|route| (id.0, route)))
                .collect();
            for (recipient, _) in &discarded {
                if let Some(counter) = routes.counters.get_mut(recipient) {
                    counter.active -= 1;
                }
            }
            routes.counters.remove(&connection);
            discarded
        };
        drop(discarded);
    }

    /// DOC-16 资源协议：按 CBOR 资源标签转交编号，Batch 保存发送完成前的拥有引用。
    pub(super) fn transfer(
        &self,
        data: Value,
        incoming: &Payload,
        recipient: &Connection,
    ) -> Result<(Value, Batch)> {
        let mut batch = self.batch(recipient.id());
        let mut translated = HashMap::new();
        let data = rewrite(data, &mut |id| {
            if let Some(mapped) = translated.get(&id) {
                return Ok(*mapped);
            }
            let object = incoming.import(id)?;
            let mapped =
                self.export_route(recipient, Route::remote(object, None, None), &mut batch)?;
            translated.insert(id, mapped);
            Ok(mapped)
        })?;
        Ok((data, batch))
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
                let (args, batch) =
                    self.transfer(Value::Array(request.args), incoming, object.peer())?;
                let Value::Array(args) = args else {
                    unreachable!("argument array preserved")
                };
                let outgoing = value(&payload::Invoke {
                    object: object.id(),
                    method: request.method,
                    args,
                    refs: batch.ids.clone(),
                })?;
                let result = object
                    .peer()
                    .request(
                        "invoke",
                        batch.outgoing(outgoing),
                        self.0.options.invoke_timeout,
                    )
                    .await?;
                let response: payload::InvokeResult = decode(result.value())?;
                let (value, batch) = self.transfer(response.value, &result, caller)?;
                let output = super::value(&payload::InvokeResult {
                    value,
                    refs: batch.ids.clone(),
                })?;
                Ok(batch.outgoing(output))
            }
            Target::Local { service, origin } => {
                let origin = origin
                    .as_ref()
                    .ok_or_else(|| error(4, "内置对象缺少调用上下文"))?;
                let objects = request
                    .refs
                    .iter()
                    .map(|id| incoming.import(*id).map(|object| (*id, object)))
                    .collect::<Result<_>>()?;
                let arguments = Arguments {
                    values: request.args,
                    objects,
                };
                let result = service.invoke(origin, &request.method, &arguments).await?;
                let (value, batch) = self.transfer(result.value, incoming, caller)?;
                let output = super::value(&payload::InvokeResult {
                    value,
                    refs: batch.ids.clone(),
                })?;
                let outgoing = batch.outgoing(output);
                Ok(match result.after_write {
                    Some(action) => outgoing.after_write(action),
                    None => outgoing,
                })
            }
        }
    }
}

fn rewrite(data: Value, resource: &mut impl FnMut(u64) -> Result<u64>) -> Result<Value> {
    Ok(match data {
        Value::Tag(payload::OBJECT_TAG, id) => {
            let id = id
                .as_integer()
                .and_then(|id| u64::try_from(id).ok())
                .filter(|id| *id != 0)
                .ok_or_else(|| error(2, "资源标签需要正u64编号"))?;
            payload::ObjectRef(resource(id)?).into()
        }
        Value::Tag(tag, value) => Value::Tag(tag, Box::new(rewrite(*value, resource)?)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| rewrite(value, resource))
                .collect::<Result<_>>()?,
        ),
        Value::Map(values) => Value::Map(
            values
                .into_iter()
                .map(|(key, value)| Ok((rewrite(key, resource)?, rewrite(value, resource)?)))
                .collect::<Result<_>>()?,
        ),
        value => value,
    })
}
