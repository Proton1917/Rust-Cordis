//! DOC-15 Q0、DOC-16：核心到原生宿主的 UP/1 请求分派、回调执行与上下文释放。
use super::*;

impl Host {
    /// DOC-16：握手建立进程身份并装入核心指定的初始原生库。
    fn hello(&self, request: payload::Hello) -> Result<Value> {
        if request.protocol != 1 {
            return Err(Error::new(3, "UP/1 主版本不匹配"));
        }
        if self.0.initialized.swap(true, Ordering::AcqRel) {
            return Err(Error::new(4, "宿主握手已执行"));
        }
        self.0.process.store(request.process, Ordering::Release);
        let _group = self.0.group.set(request.group);
        let mut plugins = Vec::new();
        for path in &self.0.options.libraries {
            let prepared = self.prepare(path)?;
            if self.0.active.lock().contains_key(&prepared.plugin.name) {
                return Err(Error::new(2, "启动列表包含同名插件库"));
            }
            self.activate(prepared.library)?;
            plugins.push(prepared.plugin);
        }
        to_value(&payload::Describe {
            protocol: 1,
            plugins,
        })
    }

    /// DOC-12 §10、DOC-15 Q0：为核心分配的加载纪元保留配置与描述符，再轮询 apply。
    async fn apply(&self, request: payload::Apply) -> Result<Value> {
        let component = self.component(&request.plugin, &request.component, request.library)?;
        let config = encode(&request.config)?;
        let inherited = request
            .inherited
            .map(|state| {
                Ok::<_, Error>(State {
                    schema: state.schema,
                    version: state.version,
                    data: encode(&state.data)?,
                })
            })
            .transpose()?;
        let record = Arc::new(ContextRecord::new(Arc::new(ContextData {
            base: request.context,
            retiring: AtomicBool::new(false),
            _component: self.0.executor.own(component.clone()),
            config: config.clone(),
            inherited,
        })));
        {
            let mut contexts = self.0.contexts.lock();
            if contexts.contains_key(&request.context) {
                return Err(Error::new(2, "上下文编号重复"));
            }
            contexts.insert(request.context, record);
        }
        component.apply(request.context, &config)?.await?;
        Ok(Value::Null)
    }

    /// DOC-16 资源协议：参数导入和结果导出分别持有调用所需资源。
    async fn invoke(&self, mut params: Payload) -> Result<Outgoing> {
        let request = payload::Invoke::from_value_owned(params.take_value())
            .map_err(|problem| Error::new(2, problem.to_string()))?;
        let object = self.objects().borrow(request.object)?;
        let bridge = self.bridge()?;
        let arguments = bridge.incoming(&params, Value::Array(request.args)).await?;
        let future = object.invoke_async(&request.method, arguments.as_slice())?;
        arguments.release()?;
        let result = future.await?;
        let response = bridge.outgoing(decode_value(result.as_slice())?)?;
        result.release()?;
        Ok(response)
    }

    /// DOC-16：返回原生对象的接口和同组直接调用标识。
    fn object_info(&self, request: payload::ObjectInfoRequest) -> Result<Value> {
        let info = self.objects().info(request.object)?;
        to_value(&payload::ObjectInfo {
            interface: info.interface,
            methods: info.methods,
            local: Some(payload::LocalObject {
                process: self.0.process.load(Ordering::Acquire),
                object: request.object,
            }),
        })
    }

    /// DOC-12 §9：释放导出器缓冲区后，将状态值返回核心。
    fn export(&self, request: payload::Export) -> Result<Value> {
        let result = self.objects().export_state(request.object)?;
        let value = decode_value(result.as_slice())?;
        result.release()?;
        to_value(&payload::ValueResult { value })
    }

    async fn callback(
        &self,
        request: payload::Callback,
        payload: Payload,
        object: Arc<Owned<Object>>,
    ) -> Result<()> {
        let bytes = self.bridge()?.incoming(&payload, request.payload).await?;
        let result = match object.header().kind {
            2 => object.call_sync(request.context, request.scope, bytes.as_slice()),
            3 => object
                .call_async(request.context, request.scope, bytes.as_slice())?
                .await
                .map(|_| ()),
            _ => Err(Error::new(2, "对象对应服务或导出器，回调入口无效")),
        };
        bytes.release()?;
        result
    }

    /// DOC-15 Q0、DOC-16：按协议方法转发核心已作出的生命周期与调用决定。
    async fn execute(&self, method: String, params: Payload) -> Result<Outgoing> {
        let reply = match method.as_str() {
            "hello" => self.hello(from_value(params.value())?)?,
            "resolve" => {
                let request: payload::Resolve = from_value(params.value())?;
                self.component(&request.plugin, &request.component, request.library)?
                    .resolve(&encode(&request.config)?)?;
                Value::Null
            }
            "apply" => self.apply(from_value(params.value())?).await?,
            "callback" => {
                let request: payload::Callback = from_value(params.value())?;
                let object = self.objects().borrow(request.object)?;
                self.callback(request, params, object).await?;
                return Ok(Value::Null.into());
            }
            "invoke" => return self.invoke(params).await,
            "object_info" => self.object_info(from_value(params.value())?)?,
            "export" => self.export(from_value(params.value())?)?,
            "thaw" => {
                let request: payload::Thaw = from_value(params.value())?;
                self.objects().thaw(request.object)?;
                Value::Null
            }
            "library_prepare" => {
                let request: payload::LibraryPrepare = from_value(params.value())?;
                to_value(&self.prepare(Path::new(&request.path))?)?
            }
            "library_status" => to_value(&self.library_status())?,
            "library_activate" => {
                let request: payload::LibraryHandle = from_value(params.value())?;
                self.activate(request.library)?;
                Value::Null
            }
            "library_retire" => {
                let request: payload::LibraryHandle = from_value(params.value())?;
                to_value(&self.retire(request.library).await?)?
            }
            "shutdown" => {
                self.shutdown().await?;
                Value::Null
            }
            _ => return Err(Error::new(3, format!("宿主请求方法 {method} 无对应入口"))),
        };
        Ok(reply.into())
    }

    /// DOC-16：通知回调在原生执行器运行，关闭上下文时取消并等待该批任务。
    fn notify_callback(&self, params: Payload) -> Result<()> {
        let request: payload::Callback = from_value(params.value())?;
        let record = {
            let contexts = self.0.contexts.lock();
            match contexts.get(&request.context) {
                Some(record) => record.clone(),
                _ => return Ok(()),
            }
        };
        let (id, task, done) = {
            let mut notifications = record.notifications.lock();
            if !matches!(*record.phase.borrow(), ClosePhase::Open)
                || self.0.stopping.load(Ordering::Acquire)
            {
                return Ok(());
            }
            let object = self.objects().borrow(request.object)?;
            let permit = self.0.callback_admission.acquire(params.wire_bytes())?;
            let host = self.clone();
            let task = self.0.executor.start(
                permit.run(async move { host.callback(request, params, object).await }),
                true,
            );
            let cancel = task.cancellation();
            let id = self.0.next_notification.fetch_add(1, Ordering::Relaxed);
            let (done, receiver) = tokio::sync::oneshot::channel();
            notifications.insert(
                id,
                NotificationTask {
                    cancel,
                    done: receiver,
                },
            );
            (id, task, done)
        };
        let host = self.clone();
        self.0.runtime.spawn(async move {
            let result = task.await;
            record.notifications.lock().remove(&id);
            let _receiver = done.send(());
            if let Err(error) = result
                && error.code() != 7
            {
                abi::environment().report(error);
            }
            drop(host);
        });
        Ok(())
    }

    /// ADR-0025：释放基上下文时同时退役其全部派生视图。
    fn release_context(&self, context: u64) {
        let records = {
            let contexts = self.0.contexts.lock();
            match contexts.get(&context) {
                Some(record) if record.data.base == context => {
                    record.data.retiring.store(true, Ordering::Release);
                    contexts
                        .iter()
                        .filter(|(_, candidate)| {
                            candidate.data.base == context && candidate.schedule_close()
                        })
                        .map(|(id, record)| (*id, record.clone()))
                        .collect::<Vec<_>>()
                }
                Some(record) if record.schedule_close() => vec![(context, record.clone())],
                _ => Vec::new(),
            }
        };
        for (id, record) in records {
            let host = self.clone();
            let task = self
                .0
                .executor
                .start(async move { host.close_context(id, record).await }, false);
            self.0.runtime.spawn(async move {
                if let Err(error) = task.await {
                    abi::environment().report(error);
                }
            });
        }
    }
}

impl Handler for Host {
    fn immediate_notification(&self, method: &str) -> bool {
        matches!(method, "callback" | "release" | "release_context")
    }

    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, std::result::Result<Outgoing, WireError>> {
        self.0.connection.get_or_init(|| peer);
        if method == "shutdown" {
            self.0.stopping.store(true, Ordering::Release);
        }
        let host = self.clone();
        let cancellable = method == "invoke";
        Box::pin(async move {
            host.0
                .executor
                .clone()
                .run(
                    cordis_up::rpc::inherit_invocation(async move {
                        host.execute(method, params).await
                    }),
                    cancellable,
                )
                .await
                .map_err(to_wire)
        })
    }

    fn notify(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, std::result::Result<(), WireError>> {
        self.0.connection.get_or_init(|| peer);
        Box::pin(async move {
            match method.as_str() {
                "channel_frame" | "channel_close" => {
                    self.0
                        .forward
                        .clone()
                        .receive(self.clone(), &method, params)
                        .await
                        .map_err(to_wire)?;
                }
                "callback" => self.notify_callback(params).map_err(to_wire)?,
                "release" => {
                    let request: payload::Release = from_value(params.value()).map_err(to_wire)?;
                    for id in request.objects {
                        self.objects().release(id).map_err(to_wire)?;
                    }
                }
                "release_context" => {
                    let request: payload::Context = from_value(params.value()).map_err(to_wire)?;
                    self.release_context(request.context);
                }
                _ => return Err(WireError::new(3, format!("宿主通知 {method} 无对应入口"))),
            }
            Ok(())
        })
    }
}
