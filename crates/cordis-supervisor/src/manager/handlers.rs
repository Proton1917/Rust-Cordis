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

fn request<T: DeserializeOwned>(connection: &Connection, params: &Payload) -> Result<T> {
    decode(params.value()).inspect_err(|_| {
        connection.terminate("UP/1方法参数结构错误");
    })
}

impl Handler for Manager {
    fn immediate_notification(&self, method: &str) -> bool {
        matches!(method, "release" | "effect_release")
    }

    fn request(
        &self,
        connection: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing>> {
        Box::pin(async move {
            let output = match method.as_str() {
                "provide" => {
                    let request: payload::Provide = request(&connection, &params)?;
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
                        &connection,
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
                        Some(self.retain_info(&connection, info)?),
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
                                _registration: Some(registration),
                                _metadata: Some(metadata),
                            }),
                        )
                        .map_err(kernel_error)?;
                    Value::Null
                }
                "get" => {
                    let request: payload::Get = request(&connection, &params)?;
                    let generation = self.generation(connection.id(), request.context)?;
                    let route = self.get_service(&generation, &request.interface)?;
                    let info = self.object_info(&route).await?;
                    let mut batch = self.batch(connection.id());
                    let object = self.export_route(&connection, route, &mut batch)?;
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
                    let request: payload::Invoke = request(&connection, &params)?;
                    return self.invoke(&connection, request, &params).await;
                }
                "object_info" => {
                    let request: payload::ObjectInfoRequest = request(&connection, &params)?;
                    let route = self.route(connection.id(), request.object)?;
                    value(&self.object_info(&route).await?)?
                }
                "effect" => {
                    let request: payload::Effect = request(&connection, &params)?;
                    let generation = self.generation(connection.id(), request.context)?;
                    let callback = params.import(request.body)?;
                    return self
                        .create_effect(generation, request.label, callback, request.asynchronous)
                        .await;
                }
                "defer" => {
                    let request: payload::Defer = request(&connection, &params)?;
                    self.defer(
                        connection.id(),
                        request.scope,
                        params.import(request.callback)?,
                    )?;
                    Value::Null
                }
                "effect_settled" | "effect_dispose" => {
                    let request: payload::EffectHandle = request(&connection, &params)?;
                    self.effect_wait(connection.id(), request.effect, method == "effect_dispose")
                        .await?;
                    Value::Null
                }
                "effect_release" => {
                    let request: payload::EffectHandle = request(&connection, &params)?;
                    self.effect_release(connection.id(), request.effect)?;
                    Value::Null
                }
                "is_current" => {
                    let request: payload::Context = request(&connection, &params)?;
                    value(
                        &self
                            .generation(connection.id(), request.context)?
                            .is_current(),
                    )?
                }
                "config" => {
                    let request: payload::Context = request(&connection, &params)?;
                    let generation = self.generation(connection.id(), request.context)?;
                    value(&payload::ValueResult {
                        value: generation.config.clone(),
                    })?
                }
                "inherited_state" => {
                    let request: payload::Context = request(&connection, &params)?;
                    value(&self.generation(connection.id(), request.context)?.inherited)?
                }
                "on" => {
                    let request: payload::On = request(&connection, &params)?;
                    let generation = self.generation(connection.id(), request.context)?;
                    self.listen(
                        generation,
                        request.event,
                        params.import(request.callback)?,
                        request.asynchronous,
                    )?;
                    Value::Null
                }
                "emit" => {
                    let request: payload::Emit = request(&connection, &params)?;
                    let generation = self.generation(connection.id(), request.context)?;
                    self.emit(
                        &generation.context,
                        request.event,
                        request.payload,
                        Some(Arc::new(params)),
                        request.asynchronous,
                    )
                    .await?;
                    Value::Null
                }
                "export_state" => {
                    let request: payload::ExportState = request(&connection, &params)?;
                    let generation = self.generation(connection.id(), request.context)?;
                    generation.active()?;
                    if generation.component.info.state_schema != request.schema {
                        return Err(error(2, "状态模式与组件声明不符"));
                    }
                    let registration = generation.reserve()?;
                    let metadata = self.retain_metadata(&connection, request.schema.len())?;
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
                    Value::Null
                }
                "log" => {
                    self.log(&connection, request(&connection, &params)?)?;
                    Value::Null
                }
                _ => return Err(error(3, format!("未知宿主方法 {method}"))),
            };
            Ok(Outgoing::new(output))
        })
    }

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
                "emit" => {
                    let request: payload::Emit = request(&connection, &params)?;
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
                _ => Err(error(3, format!("未知插件通知 {method}"))),
            }
        })
    }
}

impl Manager {
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

    pub fn emit_builtin(&self, event: &str, data: Value) -> Result<()> {
        payload::persistent_value(&data).map_err(|problem| error(2, problem.to_string()))?;
        let key = Interface::parse(event)?.key;
        let data: cordis::AnyArc = Arc::new(EventData {
            data,
            incoming: None,
        });
        self.0.context.emit_named(key, &data).map_err(kernel_error)
    }
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
