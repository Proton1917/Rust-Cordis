//! DOC-16 §5、DOC-15 §6.3/§6.4：把连接请求交给唯一内核的服务、效应和视图操作。
//! 参数结构错误关闭来源连接；有效请求的业务错误随响应返回，资源持有关系随请求或效应释放。
use super::*;
use generation::{Binding, Exporter, Registration};

struct EventData {
    data: Value,
    incoming: Option<Arc<Payload>>,
}
struct Listener {
    manager: Manager,
    view: ViewLease,
    callback: RemoteObject,
    _registration: Arc<Registration>,
    _metadata: Arc<MetadataLease>,
}

/// CP-8：方法边界解析已受限的 CBOR 值，结构错误使当前连接结束。
fn request<T: DeserializeOwned>(connection: &Connection, params: &Payload) -> Result<T> {
    decode(params.value()).inspect_err(|_| {
        connection.terminate("UP/1方法参数结构错误");
    })
}

impl Handler for Manager {
    /// DOC-16 §6：纯编号释放在读消息任务完成，先于后续请求观察释放结果。
    fn immediate_notification(&self, method: &str) -> bool {
        let extension = self
            .0
            .handler_extension
            .lock()
            .as_ref()
            .and_then(Weak::upgrade);
        matches!(method, "release" | "effect_release")
            || extension.is_some_and(|extension| extension.immediate_notification(method))
    }

    /// DOC-16 §5：按资源职责分派宿主请求，业务错误保留协议错误码返回调用方。
    fn request(
        &self,
        connection: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing>> {
        Box::pin(async move {
            let output = match method.as_str() {
                "provide" | "get" | "invoke" | "object_info" => {
                    return self.service_request(&connection, &method, &params).await;
                }
                "effect" | "defer" | "effect_settled" | "effect_dispose" | "effect_release" => {
                    return self.effect_request(&connection, &method, &params).await;
                }
                "is_current" | "config" | "inherited_state" | "context_derive" | "plugin" => {
                    return self.context_request(&connection, &method, &params).await;
                }
                "on" | "emit" => {
                    return self.event_request(&connection, &method, params).await;
                }
                "export_state" => {
                    self.register_exporter(&connection, &params)?;
                    Value::Null
                }
                "log" => {
                    self.log(&connection, request(&connection, &params)?)?;
                    Value::Null
                }
                _ => {
                    let extension = self
                        .0
                        .handler_extension
                        .lock()
                        .as_ref()
                        .and_then(Weak::upgrade);
                    return match extension {
                        Some(extension) => extension.request(connection, method, params).await,
                        None => Err(error(3, format!("未知宿主方法 {method}"))),
                    };
                }
            };
            Ok(Outgoing::new(output))
        })
    }

    /// DOC-16 §5/§6：处理释放、日志和事件通知，非法方法由连接任务报告。
    fn notify(
        &self,
        connection: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<()>> {
        Box::pin(async move {
            match method.as_str() {
                "release" => {
                    let request: payload::Release = request(&connection, &params)?;
                    self.release(connection.id(), &request.objects)
                }
                "effect_release" => {
                    let request: payload::EffectHandle = request(&connection, &params)?;
                    self.effect_release(connection.id(), request.effect)
                }
                "log" => self.log(&connection, request(&connection, &params)?),
                "emit" => self.emit_remote(&connection, params).await,
                _ => {
                    let extension = self
                        .0
                        .handler_extension
                        .lock()
                        .as_ref()
                        .and_then(Weak::upgrade);
                    match extension {
                        Some(extension) => extension.notify(connection, method, params).await,
                        None => Err(error(3, format!("未知插件通知 {method}"))),
                    }
                }
            }
        })
    }
}

impl Manager {
    /// DOC-16 §5、DOC-15 §6.4：服务登记、取得与调用沿当前上下文路由，响应提交时交接对象编号。
    async fn service_request(
        &self,
        connection: &Connection,
        method: &str,
        params: &Payload,
    ) -> Result<Outgoing> {
        let output = match method {
            "provide" => {
                self.provide_remote(connection, params)?;
                Value::Null
            }
            "get" => {
                let request: payload::Get = request(connection, params)?;
                let generation = self.generation(connection.id(), request.context)?;
                let route = self.get_service(&generation, &request.interface).await?;
                let info = self.object_info(&route).await?;
                let mut batch = self.batch(connection.id());
                let object = self.export_route(connection, route, &mut batch)?;
                let result = value(&payload::GetResult {
                    object,
                    interface: info.interface,
                    methods: info.methods,
                    local: info.local,
                    refs: batch.ids.clone(),
                })?;
                return Ok(batch.outgoing(result));
            }
            "invoke" => {
                return self
                    .invoke(connection, request(connection, params)?, params)
                    .await;
            }
            "object_info" => {
                let request: payload::ObjectInfoRequest = request(connection, params)?;
                let route = self.route(connection.id(), request.object)?;
                value(&self.object_info(&route).await?)?
            }
            _ => return Err(error(3, format!("未知服务方法 {method}"))),
        };
        Ok(Outgoing::new(output))
    }

    /// DOC-16 §5、PR-1：在活跃视图中登记描述符声明的服务，额度与远端对象随内核绑定回退。
    fn provide_remote(&self, connection: &Connection, params: &Payload) -> Result<()> {
        let request: payload::Provide = request(connection, params)?;
        let generation = self.generation(connection.id(), request.context)?;
        generation.active()?;
        let interface = generation
            .component
            .provides
            .iter()
            .find(|interface| interface.full == request.interface)
            .cloned()
            .ok_or_else(|| error(2, "提供接口需要在组件描述符中声明"))?;
        let registration = generation.reserve()?;
        let metadata = self.retain_metadata(
            connection,
            metadata::strings([interface.full.as_str(), interface.key.as_str()])?,
        )?;
        let object = params.import(request.object)?;
        let info = payload::ObjectInfo {
            interface: interface.full.clone(),
            methods: request.methods,
            local: Some(payload::LocalObject {
                process: connection.id(),
                object: request.object,
            }),
        };
        let route = Route::remote(
            object,
            Some(self.retain_info(connection, info)?),
            Some(Arc::downgrade(&generation.generation)),
        );
        generation
            .context
            .provide_named(
                interface.key.clone(),
                Arc::new(Binding {
                    interface,
                    route,
                    generation: Some(generation.generation.clone()),
                    scoped: request.scoped,
                    _view: Some(generation.record.clone()),
                    _registration: Some(registration),
                    _metadata: Some(metadata),
                }),
            )
            .map_err(kernel_error)?;
        Ok(())
    }

    /// DOC-16 §5、T-EFF-2：效应与清理登记到内核，稳定和释放操作沿所属连接查找句柄。
    async fn effect_request(
        &self,
        connection: &Connection,
        method: &str,
        params: &Payload,
    ) -> Result<Outgoing> {
        match method {
            "effect" => {
                let request: payload::Effect = request(connection, params)?;
                let generation = self.generation(connection.id(), request.context)?;
                let callback = params.import(request.body)?;
                return self
                    .create_effect(generation, request.label, callback, request.asynchronous)
                    .await;
            }
            "defer" => {
                let request: payload::Defer = request(connection, params)?;
                self.defer(
                    connection.id(),
                    request.scope,
                    params.import(request.callback)?,
                )?;
            }
            "effect_settled" | "effect_dispose" => {
                let request: payload::EffectHandle = request(connection, params)?;
                self.effect_wait(connection.id(), request.effect, method == "effect_dispose")
                    .await?;
            }
            "effect_release" => {
                let request: payload::EffectHandle = request(connection, params)?;
                self.effect_release(connection.id(), request.effect)?;
            }
            _ => return Err(error(3, format!("未知效应方法 {method}"))),
        }
        Ok(Outgoing::new(Value::Null))
    }

    /// DOC-15 §6.4、ADR-0025：视图查询保持实例归属，派生与子组件经唯一内核建立生命周期。
    async fn context_request(
        &self,
        connection: &Connection,
        method: &str,
        params: &Payload,
    ) -> Result<Outgoing> {
        let output = match method {
            "is_current" | "config" | "inherited_state" => {
                let request: payload::Context = request(connection, params)?;
                let generation = self.generation(connection.id(), request.context)?;
                match method {
                    "is_current" => value(&generation.is_current())?,
                    "config" => value(&payload::ValueResult {
                        value: generation.config.clone(),
                    })?,
                    _ => value(&generation.inherited)?,
                }
            }
            "context_derive" => {
                return self.derive_remote(connection, request(connection, params)?);
            }
            "plugin" => {
                let request: payload::PluginRequest = request(connection, params)?;
                let parent = self.generation(connection.id(), request.context)?;
                let instance = self
                    .mount_child(parent, request.component, request.config)
                    .await?;
                value(&payload::PluginResult { instance })?
            }
            _ => return Err(error(3, format!("未知上下文方法 {method}"))),
        };
        Ok(Outgoing::new(output))
    }

    /// DOC-15 §6.3：事件监听归属当前视图，远端监听器以通知执行。
    async fn event_request(
        &self,
        connection: &Connection,
        method: &str,
        params: Payload,
    ) -> Result<Outgoing> {
        match method {
            "on" => {
                let request: payload::On = request(connection, &params)?;
                let generation = self.generation(connection.id(), request.context)?;
                self.listen(
                    generation,
                    request.event,
                    params.import(request.callback)?,
                    request.asynchronous,
                )?;
            }
            "emit" => self.emit_remote(connection, params).await?,
            _ => return Err(error(3, format!("未知事件方法 {method}"))),
        }
        Ok(Outgoing::new(Value::Null))
    }

    /// DOC-15 §6.3、DOC-16 §6：请求和通知共用事件分发，输入资源保留至事件传递完成。
    async fn emit_remote(&self, connection: &Connection, params: Payload) -> Result<()> {
        let request: payload::Emit = request(connection, &params)?;
        let generation = self.generation(connection.id(), request.context)?;
        self.emit(
            &generation.context,
            request.event,
            request.payload,
            Some(Arc::new(params)),
            request.asynchronous,
        )
        .await
    }

    /// DOC-11 §5、SW-5：导出器模式匹配声明后作为效应登记，重复登记返回错误，回退释放持有资源。
    fn register_exporter(&self, connection: &Connection, params: &Payload) -> Result<()> {
        let request: payload::ExportState = request(connection, params)?;
        let generation = self.generation(connection.id(), request.context)?;
        generation.active()?;
        if generation.component.info.state_schema != request.schema {
            return Err(error(2, "状态模式与组件声明不符"));
        }
        let registration = generation.reserve()?;
        let metadata = self.retain_metadata(connection, request.schema.len())?;
        let exporter = Arc::new(Exporter {
            schema: request.schema,
            version: request.version,
            context: generation.id,
            object: params.import(request.exporter)?,
        });
        let retained = generation.clone();
        generation
            .context
            .effect("remote-exporter", move |scope| {
                {
                    let mut slot = retained.exporter.lock();
                    if slot.is_some() {
                        return Err(cordis::Error::new("本纪元已有状态导出器"));
                    }
                    *slot = Some(exporter);
                }
                scope.defer(move || {
                    let exporter = retained.exporter.lock().take();
                    drop((exporter, registration, metadata));
                    Ok(())
                })
            })
            .map_err(kernel_error)?;
        Ok(())
    }

    /// CP-7、S-LOG：将子进程诊断归属连接并截取消息，日志接收器在锁外执行。
    pub(super) fn stderr(&self, connection: u64, line: String) {
        let record = LogRecord {
            origin: LogOrigin::Stderr,
            connection,
            context: 0,
            instance: format!("connection:{connection}"),
            level: 1,
            message: metadata::clipped(&line),
        };
        let sink = self.0.log_sink.lock().clone();
        if let Some(sink) = sink {
            sink(record);
        } else {
            eprintln!("[stderr {}] {}", record.instance, record.message);
        }
    }

    /// DOC-15 §6.3：内置事件接受持久值，由核心事件系统选择当前监听器。
    pub fn emit_builtin(&self, event: &str, data: Value) -> Result<()> {
        payload::persistent_value(&data).map_err(|problem| error(2, problem.to_string()))?;
        let key = Interface::parse(event)?.key;
        let data: cordis::AnyArc = Arc::new(EventData {
            data,
            incoming: None,
        });
        self.0.context.emit_named(key, &data).map_err(kernel_error)
    }
    /// S-LOG、CP-2：校验协议日志边界后按上下文归属送达日志服务，超限返回参数错误。
    fn log(&self, connection: &Connection, request: payload::Log) -> Result<()> {
        if request.message.len() > metadata::LOG_BYTES {
            return Err(error(2, "日志消息超过4096字节"));
        }
        if request.level > 3 {
            return Err(error(2, "日志级别需要为0到3"));
        }
        let logger = if request.context == 0 {
            self.0
                .context
                .logger()
                .named(format!("connection:{}", connection.id()))
        } else {
            let current = self.generation(connection.id(), request.context)?;
            current
                .context
                .logger()
                .named(current.instance.name.clone())
        };
        let sink = self.0.log_sink.lock().clone();
        if let Some(sink) = sink {
            sink(LogRecord {
                origin: LogOrigin::Protocol,
                connection: connection.id(),
                context: request.context,
                instance: logger.name().into(),
                level: request.level,
                message: request.message,
            });
            return Ok(());
        }
        match request.level {
            0 => logger.error(request.message),
            1 => logger.warn(request.message),
            2 => logger.info(request.message),
            3 => logger.debug(request.message),
            _ => return Err(error(2, "日志级别需要为0到3")),
        }
        Ok(())
    }

    /// DOC-15 §6.3：活跃视图持有远端监听器和额度，清理由所属内核效应驱动。
    fn listen(
        &self,
        generation: ViewLease,
        event: String,
        callback: RemoteObject,
        asynchronous: bool,
    ) -> Result<()> {
        generation.active()?;
        let key = Interface::parse(&event)?.key;
        let registration = generation.reserve()?;
        let metadata = self.retain_metadata(&generation.component.connection, key.len())?;
        let context = generation.context.clone();
        let listener = Arc::new(Listener {
            manager: self.clone(),
            view: generation,
            callback,
            _registration: registration,
            _metadata: metadata,
        });
        if asynchronous {
            context
                .on_named_async(
                    key,
                    move |data| {
                        let listener = listener.clone();
                        Box::pin(async move {
                            listener.dispatch(&data);
                            Ok(None)
                        })
                    },
                    cordis::EventOptions::default(),
                )
                .map_err(kernel_error)?;
        } else {
            context
                .on_named(key, move |data| {
                    listener.dispatch(data);
                    Ok(None)
                })
                .map_err(kernel_error)?;
        }
        Ok(())
    }

    /// DOC-15 §6.3：按内核事件类别派发，远端监听器统一由 Listener 发送通知。
    async fn emit(
        &self,
        context: &Context,
        event: String,
        data: Value,
        incoming: Option<Arc<Payload>>,
        asynchronous: bool,
    ) -> Result<()> {
        let key = Interface::parse(&event)?.key;
        let data: cordis::AnyArc = Arc::new(EventData { data, incoming });
        if asynchronous {
            context
                .parallel_named(key, data)
                .await
                .map_err(kernel_error)
        } else {
            context.emit_named(key, &data).map_err(kernel_error)
        }
    }
}

impl Listener {
    /// CP-3、DOC-16 §6：把事件资源交接到有界通知队列，投递错误写入所属视图日志。
    fn dispatch(&self, data: &cordis::AnyArc) {
        if !self.callback.peer().is_alive() {
            return;
        }
        let Some(data) = data.downcast_ref::<EventData>() else {
            self.view.context.logger().error("远端事件载荷类型错误");
            return;
        };
        let result = (|| {
            let (data, batch) = match &data.incoming {
                Some(incoming) => {
                    self.manager
                        .transfer(data.data.clone(), incoming, self.callback.peer())?
                }
                None => (
                    data.data.clone(),
                    self.manager.batch(self.callback.peer().id()),
                ),
            };
            let params = value(&payload::Callback {
                object: self.callback.id(),
                context: self.view.id,
                scope: 0,
                payload: data,
                refs: batch.ids.clone(),
            })?;
            self.callback
                .peer()
                .try_notify("callback", batch.outgoing(params))
        })();
        if let Err(problem) = result {
            self.view
                .context
                .logger()
                .warn(saved_error(&problem).to_string());
        }
    }
}
