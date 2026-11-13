//! DOC-15 §11.2–11.4：页面子实例归属真实父效应，连接关闭回收自己的页面组。

use super::*;
use cordis_supervisor::manager::InstanceSpec;
use std::path::PathBuf;

impl Deployment {
    /// DOC-15 §11.3：登记及清理先建立拥有关系，再把模块路由发布给页面。
    pub(super) async fn activate(self: Arc<Self>, context: Context, mut spec: Spec) -> Result<()> {
        if let Some(parent) = &spec.parent {
            let component = self
                .manager
                .snapshot()
                .instances
                .into_iter()
                .find(|instance| instance.name == *parent)
                .ok_or_else(|| WireError::new(4, "网页后端实例目录缺席"))?
                .component;
            spec.component = format!("{component}/web");
        }
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        let registration = registry::Registration::create(&self.manager, &spec, sequence).await?;
        let children = Arc::new(Mutex::new(BTreeMap::<u64, Fiber>::new()));
        let live = Arc::new(Live {
            spec: spec.clone(),
            context: context.clone(),
            registration: registration.clone(),
            children: children.clone(),
            mounting: Mutex::new(HashMap::new()),
        });
        let weak = self.weak.clone();
        let manager = self.manager.clone();
        self.live.lock().insert(spec.instance.clone(), live.clone());
        let retained = live.clone();
        let effect = context.effect("web-resources", move |scope| {
            scope.defer_async(move || release_registration(weak, manager, retained))
        });
        if let Err(problem) = effect {
            release_registration(self.weak.clone(), self.manager.clone(), live)
                .await
                .map_err(wire)?;
            return Err(wire(problem));
        }
        if !context.is_current() {
            return Err(WireError::new(4, "网页资源Owner已进入清理"));
        }
        let pages = self
            .pages
            .lock()
            .iter()
            .map(|(id, page)| (*id, page.clone()))
            .collect::<Vec<_>>();
        for (id, page) in pages {
            self.mount_page(id, &page, &live).await?;
        }
        live.registration.activate().await
    }

    pub(super) fn child_names(&self, instance: &str) -> Vec<String> {
        self.live
            .lock()
            .get(instance)
            .map(|live| {
                live.children
                    .lock()
                    .keys()
                    .map(|page| child_name(*page, instance))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// ADR-0041：首次加入页面使用add_library，既有模块使用已有组内替换事务。
    async fn module(&self, page: &Page, live: &Live) -> Result<()> {
        let previous = page.plugins.lock().get(&live.spec.name).cloned();
        let path = PathBuf::from(&live.registration.path);
        match previous {
            None => {
                let plugin = self.manager.add_library(page.group.clone(), path).await?;
                page.provides.lock().extend(
                    plugin
                        .components
                        .iter()
                        .flat_map(|component| component.provides.clone()),
                );
            }
            Some((_, previous_path)) if previous_path != live.registration.version_path() => {
                self.manager
                    .replace_library(page.group.clone(), live.spec.name.clone(), path)
                    .await?;
            }
            Some(_) => {}
        }
        page.plugins.lock().insert(
            live.spec.name.clone(),
            (
                live.spec.version.clone(),
                live.registration.version_path().into(),
            ),
        );
        Ok(())
    }

    /// DOC-15 §11.2：页面提供的服务及敏感核心能力进入页面私有域，后端普通服务保持声明注入。
    async fn mount_page(&self, id: u64, page: &Page, live: &Live) -> Result<()> {
        if !page.connection.is_alive() || !live.context.is_current() {
            return Ok(());
        }
        let (ready, first) = {
            let mut mounting = live.mounting.lock();
            match mounting.get(&id) {
                Some(ready) => (ready.clone(), false),
                None => {
                    let ready = watch::channel(None).0;
                    mounting.insert(id, ready.clone());
                    (ready, true)
                }
            }
        };
        if !first {
            let mut received = ready.subscribe();
            loop {
                if let Some(result) = received.borrow_and_update().clone() {
                    return result;
                }
                received
                    .changed()
                    .await
                    .map_err(|_| WireError::new(4, "页面挂载结果已结束"))?;
            }
        }
        let result = self.mount_page_once(id, page, live).await;
        ready.send_replace(Some(result.clone()));
        result
    }

    /// DOC-15 §11.3：在当前后端上下文创建页面子实例，父级结束时释放登记。
    async fn mount_page_once(&self, id: u64, page: &Page, live: &Live) -> Result<()> {
        let parent = match &live.spec.parent {
            Some(parent) => self.manager.active_instance_context(parent)?,
            None => live.context.clone(),
        };
        let parent = self.manager.page_context(&parent, id);
        self.module(page, live).await?;
        if !live.context.is_current() {
            return Ok(());
        }
        let mut interfaces = page.provides.lock().clone();
        interfaces.extend([
            "cordis.manage@1".into(),
            "cordis.forward@1".into(),
            "cordis.web@1".into(),
        ]);
        interfaces.sort();
        interfaces.dedup();
        let options = payload::ContextOptions {
            isolate: interfaces
                .into_iter()
                .map(|interface| payload::Isolation {
                    interface,
                    realm: Some(format!("cordis-page-{id}")),
                })
                .collect(),
            intercept: Vec::new(),
        };
        let name = child_name(id, &live.spec.instance);
        let inherited = self.saved.lock().remove(&name);
        let specification = InstanceSpec {
            name: name.clone(),
            group: page.group.clone(),
            plugin: live.spec.name.clone(),
            component: live.spec.component.clone(),
            config: live.spec.config.clone(),
        };
        let fiber = self
            .manager
            .mount_owned_child(parent, specification, inherited, options)
            .await?;
        let retained = {
            let mut children = live.children.lock();
            if page.connection.is_alive() && live.context.is_current() {
                children.insert(id, fiber.clone());
                true
            } else {
                false
            }
        };
        if !retained {
            self.manager.dispose_owned_child(&name, &fiber).await?;
            return Ok(());
        }
        fiber.settled().await.map_err(wire)
    }

    /// DOC-16 §11：每个页面独立握手，描述仅接受当前发布的插件身份。
    async fn page(self: Arc<Self>, connection: Connection) -> Result<()> {
        let id = connection.id();
        let group = format!("web-page-{id}");
        let snapshot = self
            .manager
            .attach_connection(group.clone(), connection.clone())
            .await?;
        let live = self.live.lock().values().cloned().collect::<Vec<_>>();
        for plugin in &snapshot.plugins {
            if !live
                .iter()
                .any(|entry| entry.spec.name == plugin.name && entry.spec.version == plugin.version)
            {
                connection.terminate("页面描述包含已撤回的插件版本");
                self.manager.detach_group(&group).await?;
                return Err(WireError::new(3, "页面插件描述与活动清单不符"));
            }
        }
        let page = Page {
            group: group.clone(),
            connection: connection.clone(),
            plugins: Arc::new(Mutex::new(
                snapshot
                    .plugins
                    .iter()
                    .map(|plugin| {
                        (
                            plugin.name.clone(),
                            (plugin.version.clone(), plugin.build_info.clone()),
                        )
                    })
                    .collect(),
            )),
            provides: Arc::new(Mutex::new(
                snapshot
                    .plugins
                    .iter()
                    .flat_map(|plugin| &plugin.components)
                    .flat_map(|component| component.provides.clone())
                    .collect(),
            )),
        };
        self.pages.lock().insert(id, page.clone());
        let current = self.live.lock().values().cloned().collect::<Vec<_>>();
        let mounted = async {
            for entry in &current {
                self.mount_page(id, &page, entry).await?;
            }
            Ok::<_, WireError>(())
        }
        .await;
        if let Err(problem) = &mounted {
            connection.terminate(&problem.to_string());
        }
        connection.closed().await;
        self.release_page(id, &group).await?;
        mounted
    }

    /// DOC-15 §11.2：连接结束后移除本页子实例，再释放逻辑组。
    async fn release_page(&self, id: u64, group: &str) -> Result<()> {
        self.pages.lock().remove(&id);
        let live = self.live.lock().values().cloned().collect::<Vec<_>>();
        for entry in live {
            entry.mounting.lock().remove(&id);
            let child = entry.children.lock().remove(&id);
            if let Some(fiber) = child {
                self.manager
                    .dispose_owned_child(&child_name(id, &entry.spec.instance), &fiber)
                    .await?;
            }
        }
        self.manager.detach_group(group).await
    }
}

fn child_name(page: u64, instance: &str) -> String {
    format!("web-{page}-{instance}")
}

/// DOC-15 §11.3：逐项完成页面清理，再撤回原HTTP登记；失败保留首个真实错误。
async fn release_registration(
    deployment: Weak<Deployment>,
    manager: Arc<Manager>,
    live: Arc<Live>,
) -> cordis::Result<()> {
    if let Some(deployment) = deployment.upgrade() {
        let mut entries = deployment.live.lock();
        if entries
            .get(&live.spec.instance)
            .is_some_and(|entry| entry.registration.sequence == live.registration.sequence)
        {
            entries.remove(&live.spec.instance);
        }
    }
    live.mounting.lock().clear();
    let children = std::mem::take(&mut *live.children.lock());
    let mut failure = None;
    for (page, fiber) in children {
        if let Err(problem) = manager
            .dispose_owned_child(&child_name(page, &live.spec.instance), &fiber)
            .await
        {
            failure.get_or_insert(problem);
        }
    }
    if let Err(problem) = live.registration.close().await {
        failure.get_or_insert(problem);
    }
    failure.map_or(Ok(()), |problem| Err(kernel(problem)))
}

#[derive(Deserialize)]
struct Origin {
    context: u64,
    #[serde(rename = "page")]
    _page: Value,
}

impl Handler for Deployment {
    fn immediate_notification(&self, method: &str) -> bool {
        matches!(method, "channel_frame" | "channel_close")
    }
    /// DOC-16 §11：先核对真实加载上下文和配置授权，再建立逻辑连接。
    fn request(
        &self,
        parent: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing>> {
        Box::pin(async move {
            if method != "channel_open" {
                return Err(WireError::new(3, format!("网页扩展请求缺席：{method}")));
            }
            let request: payload::ChannelOpen = params.value().deserialized().map_err(wire)?;
            let origin: Origin = request.metadata.deserialized().map_err(wire)?;
            let caller = self.manager.request_origin(&parent, origin.context)?;
            if !(self.allowed)(&caller.instance) {
                return Err(WireError::new(9, "配置实例缺少forwarder授权"));
            }
            self.channels.authorize(parent.id());
            let id = self.manager.reserve_connection_id();
            let connection =
                self.channels
                    .open(parent.clone(), &request, id, self.manager.clone())?;
            self.origins
                .lock()
                .insert((parent.id(), request.channel), caller.instance);
            let deployment = self
                .weak
                .upgrade()
                .ok_or_else(|| WireError::new(4, "网页部署已结束"))?;
            tokio::spawn(async move {
                if let Err(problem) = deployment.clone().page(connection).await {
                    deployment
                        .manager
                        .context()
                        .logger()
                        .warn(problem.to_string());
                }
                deployment
                    .origins
                    .lock()
                    .remove(&(parent.id(), request.channel));
            });
            Ok(Outgoing::new(Value::Null))
        })
    }

    fn notify(
        &self,
        parent: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<()>> {
        Box::pin(async move {
            match method.as_str() {
                "channel_frame" => {
                    let frame: payload::ChannelFrame =
                        params.value().deserialized().map_err(wire)?;
                    let allowed = self
                        .origins
                        .lock()
                        .get(&(parent.id(), frame.channel))
                        .is_some_and(|instance| (self.allowed)(instance));
                    if !allowed {
                        return self.channels.close(
                            parent.id(),
                            payload::ChannelClose {
                                channel: frame.channel,
                                reason: "转发授权已撤回".into(),
                            },
                        );
                    }
                    self.channels.frame(parent.id(), frame)
                }
                "channel_close" => self
                    .channels
                    .close(parent.id(), params.value().deserialized().map_err(wire)?),
                _ => Err(WireError::new(3, format!("网页扩展通知缺席：{method}"))),
            }
        })
    }
}
