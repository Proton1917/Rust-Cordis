//! DOC-12 §9、DOC-15 §8、ADR-0025：原生插件向核心发起的上下文、效应、服务与事件操作。
use super::*;

impl Host {
    /// DOC-16：导出单个拥有对象并与当前请求绑定提交时机。
    fn send_with_object<T: serde::Serialize>(
        &self,
        method: &str,
        object: Object,
        build: impl FnOnce(u64, Vec<u64>) -> T,
    ) -> Result<Payload> {
        let mut refs = self.objects().export_batch(abi::environment());
        let id = refs.insert(object);
        let params = build(id, refs.refs().to_vec());
        self.request_sync(
            method,
            Outgoing::with_commit(to_value(&params)?, move || refs.commit()),
        )
    }

    pub fn log(&self, context: u64, level: u32, message: &str) -> Result<()> {
        if context != 0 {
            self.context(context)?;
        }
        self.request_sync(
            "log",
            to_value(&payload::Log {
                context,
                level,
                message: message.into(),
            })?,
        )?;
        Ok(())
    }

    pub fn now(&self, context: u64) -> Result<u64> {
        self.context(context)?;
        Ok(self.0.started.elapsed().as_nanos() as u64)
    }

    pub fn sleep(&self, context: u64, nanos: u64) -> Result<RemoteFuture<AbiUnit>> {
        self.context(context)?;
        self.scheduled(
            async move {
                tokio::time::sleep(Duration::from_nanos(nanos)).await;
                Ok(AbiUnit::default())
            },
            true,
        )
    }

    pub fn is_current(&self, context: u64) -> Result<bool> {
        let response = self.request_sync("is_current", to_value(&payload::Context { context })?)?;
        from_value(response.value())
    }

    pub fn config(&self, context: u64) -> Result<Vec<u8>> {
        Ok(self.context(context)?.data.config.clone())
    }

    pub fn inherited_state(&self, context: u64) -> Result<Option<State>> {
        Ok(self.context(context)?.data.inherited.clone())
    }

    /// ADR-0025：核心返回拥有租约的视图，共享其加载配置并与退役登记同步。
    pub fn context_derive(
        &self,
        context: u64,
        options: &[u8],
    ) -> Result<cordis_abi::DerivedContext> {
        let parent = self.context(context)?;
        let options: payload::ContextOptions = from_value(decode_value(options)?)?;
        let request = payload::ContextDerive {
            context,
            isolate: options.isolate,
            intercept: options.intercept,
        };
        let response = self.request_sync("context_derive", to_value(&request)?)?;
        let result: payload::ContextDerived = from_value(response.value())?;
        let reference = response.import(result.object).map_err(from_wire)?;
        let lease = self.bridge()?.import(
            reference,
            payload::ObjectInfo {
                interface: "cordis.context-view@1.0".into(),
                methods: Vec::new(),
                local: None,
            },
        )?;
        {
            let mut contexts = self.0.contexts.lock();
            if !matches!(*parent.phase.borrow(), ClosePhase::Open)
                || parent.data.retiring.load(Ordering::Acquire)
            {
                return Err(Error::new(4, "父上下文正在释放"));
            }
            if let Some(existing) = contexts.get(&result.context) {
                if existing.data.base != parent.data.base {
                    return Err(Error::new(2, "派生视图属于其他加载纪元"));
                }
            } else {
                contexts.insert(
                    result.context,
                    Arc::new(ContextRecord::new(parent.data.clone())),
                );
            }
        }
        Ok(cordis_abi::DerivedContext {
            context: result.context,
            lease,
        })
    }

    /// ADR-0025：子组件选择和装入交由核心，Future 保留请求至组件稳定。
    pub fn plugin(
        &self,
        context: u64,
        component: &str,
        config: &[u8],
    ) -> Result<RemoteFuture<u64>> {
        let host = self.clone();
        let request = payload::PluginRequest {
            context,
            component: component.into(),
            config: decode_value(config)?,
        };
        self.scheduled(
            async move {
                let response = host.request("plugin", to_value(&request)?).await?;
                Ok(from_value::<payload::PluginResult>(response.value())?.instance)
            },
            false,
        )
    }

    /// DOC-12 §9：效应回调作为拥有资源发送，核心登记成功后返回外部句柄。
    pub fn effect(
        &self,
        context: u64,
        label: &str,
        body: Object,
        asynchronous: bool,
    ) -> Result<u64> {
        let response = self.send_with_object("effect", body, |body, refs| payload::Effect {
            context,
            label: label.into(),
            body,
            asynchronous,
            refs,
        })?;
        Ok(from_value::<payload::EffectHandle>(response.value())?.effect)
    }

    pub fn defer(&self, scope: u64, cleanup: Object, asynchronous: bool) -> Result<()> {
        self.send_with_object("defer", cleanup, |callback, refs| payload::Defer {
            scope,
            callback,
            asynchronous,
            refs,
        })?;
        Ok(())
    }

    pub fn effect_wait(&self, effect: u64, dispose: bool) -> Result<RemoteFuture<AbiUnit>> {
        let host = self.clone();
        self.scheduled(
            async move {
                host.request(
                    if dispose {
                        "effect_dispose"
                    } else {
                        "effect_settled"
                    },
                    to_value(&payload::EffectHandle { effect })?,
                )
                .await?;
                Ok(AbiUnit::default())
            },
            !dispose,
        )
    }

    /// DOC-12 §9：最终 SDK 句柄释放经有界通知发送，发送失败终止失去一致性的连接。
    pub fn effect_release(&self, effect: u64) -> Result<()> {
        let connection = self.connection()?;
        if let Err(error) = connection.try_notify(
            "effect_release",
            to_value(&payload::EffectHandle { effect })?,
        ) {
            connection.terminate("效应句柄释放通知发送失败");
            return Err(from_wire(error));
        }
        Ok(())
    }

    pub fn provide(
        &self,
        context: u64,
        interface: &str,
        value: Object,
        scoped: bool,
    ) -> Result<()> {
        let methods = value.method_names()?;
        self.send_with_object("provide", value, |object, refs| payload::Provide {
            context,
            interface: interface.into(),
            object,
            methods,
            scoped,
            refs,
        })?;
        Ok(())
    }

    /// DOC-15 §8：服务授权与解析经核心，同组结果恢复本地 ABI 对象。
    pub fn get(&self, context: u64, interface: &str) -> Result<Object> {
        let response = self.request_sync(
            "get",
            to_value(&payload::Get {
                context,
                interface: interface.into(),
            })?,
        )?;
        let result: payload::GetResult = from_value(response.value())?;
        let object = response.import(result.object).map_err(from_wire)?;
        if result.interface == "cordis.forward@1.0" {
            return crate::forward::object(self.clone(), context, object);
        }
        self.bridge()?.import(
            object,
            payload::ObjectInfo {
                interface: result.interface,
                methods: result.methods,
                local: result.local,
            },
        )
    }

    pub fn on(&self, context: u64, event: &str, listener: Object) -> Result<()> {
        let asynchronous = match listener.header().kind {
            2 => false,
            3 => true,
            _ => return Err(Error::new(2, "事件监听器类型无效")),
        };
        self.send_with_object("on", listener, |callback, refs| payload::On {
            context,
            event: event.into(),
            callback,
            asynchronous,
            refs,
        })?;
        Ok(())
    }

    fn emit_message(
        &self,
        context: u64,
        event: &str,
        bytes: &[u8],
        asynchronous: bool,
    ) -> Result<Outgoing> {
        let mut payload = decode_value(bytes)?;
        let mut refs = self.objects().export_batch(abi::environment());
        refs.rewrite(&mut payload)?;
        let params = payload::Emit {
            context,
            event: event.into(),
            payload,
            asynchronous,
            refs: refs.refs().to_vec(),
        };
        Ok(Outgoing::with_commit(to_value(&params)?, move || {
            refs.commit()
        }))
    }

    pub fn emit(&self, context: u64, event: &str, bytes: &[u8], asynchronous: bool) -> Result<()> {
        self.request_sync(
            "emit",
            self.emit_message(context, event, bytes, asynchronous)?,
        )?;
        Ok(())
    }

    pub fn emit_async(
        &self,
        context: u64,
        event: &str,
        bytes: &[u8],
    ) -> Result<RemoteFuture<AbiUnit>> {
        let message = self.emit_message(context, event, bytes, true)?;
        let host = self.clone();
        self.scheduled(
            async move {
                host.request("emit", message).await?;
                Ok(AbiUnit::default())
            },
            false,
        )
    }

    pub fn export_state(
        &self,
        context: u64,
        schema: &str,
        version: u32,
        exporter: Object,
    ) -> Result<()> {
        self.send_with_object("export_state", exporter, |exporter, refs| {
            payload::ExportState {
                context,
                schema: schema.into(),
                version,
                exporter,
                refs,
            }
        })?;
        Ok(())
    }
}
