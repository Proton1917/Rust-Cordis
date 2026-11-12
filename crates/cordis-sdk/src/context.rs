use crate::{AsyncEvent, Error, Frozen, Interface, Result, Service, StateBlob, SyncEvent};
use cordis_abi::{
    AbiUnit, Environment, async_callback, make_future, state_exporter, sync_callback,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// DOC-12 §9：上下文句柄属于一次加载纪元。
#[derive(Clone)]
pub struct Context {
    environment: Environment,
    handle: u64,
}

impl Context {
    #[doc(hidden)]
    pub fn new(environment: Environment, handle: u64) -> Self {
        Self {
            environment,
            handle,
        }
    }

    pub fn log(&self, level: u32, message: &str) -> Result<()> {
        self.environment.host().log(self.handle, level, message)
    }

    pub fn now(&self) -> Result<Duration> {
        self.environment
            .host()
            .now(self.handle)
            .map(Duration::from_nanos)
    }

    pub async fn sleep(&self, duration: Duration) -> Result<()> {
        let nanos = u64::try_from(duration.as_nanos())
            .map_err(|_| Error::new(2, "等待时长超过 u64 纳秒范围"))?;
        self.environment.host().sleep(self.handle, nanos)?.await?;
        Ok(())
    }

    pub fn is_current(&self) -> Result<bool> {
        self.environment.host().is_current(self.handle)
    }

    pub fn config<T: DeserializeOwned>(&self) -> Result<T> {
        crate::decode(&self.environment.host().config(self.handle)?)
    }

    pub fn data_dir(&self) -> Result<std::path::PathBuf> {
        Ok(self.get::<dyn crate::builtin::DataDir>()?.path()?.into())
    }

    /// PR-1：宿主在实例停用期间终止并等待任务释放。
    pub fn spawn<F>(&self, task: F) -> Result<()>
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let future = make_future(
            async move {
                task.await?;
                Ok(AbiUnit::default())
            },
            self.environment,
        )?;
        self.environment.host().spawn(self.handle, future)
    }

    /// PR-2：线程检查传入上下文的纪元，并在停用期间返回。
    pub fn spawn_thread<F>(&self, name: &str, body: F) -> Result<()>
    where
        F: FnOnce(Context) -> Result<()> + Send + 'static,
    {
        let environment = self.environment;
        let body = Mutex::new(Some(body));
        let callback = sync_callback(environment, move |ctx, _, _| {
            take_once(&body)?(Context::new(environment, ctx))
        })?;
        environment.host().spawn_thread(self.handle, name, callback)
    }

    pub fn effect<F>(&self, label: &str, body: F) -> Result<Effect>
    where
        F: FnOnce(EffectScope) -> Result<()> + Send + 'static,
    {
        let environment = self.environment;
        let body = Mutex::new(Some(body));
        let callback = sync_callback(environment, move |ctx, scope, _| {
            take_once(&body)?(EffectScope {
                context: Context::new(environment, ctx),
                handle: scope,
            })
        })?;
        let handle = environment.host().effect(self.handle, label, callback)?;
        Ok(Effect::new(environment, handle))
    }

    pub fn effect_async<F, Fut>(&self, label: &str, body: F) -> Result<Effect>
    where
        F: FnOnce(EffectScope) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let environment = self.environment;
        let body = Mutex::new(Some(body));
        let callback = async_callback(environment, move |ctx, scope, _| {
            let body = take_once(&body);
            async move {
                body?(EffectScope {
                    context: Context::new(environment, ctx),
                    handle: scope,
                })
                .await
            }
        })?;
        let handle = environment
            .host()
            .effect_async(self.handle, label, callback)?;
        Ok(Effect::new(environment, handle))
    }

    pub fn provide<I: Interface + ?Sized>(&self, value: Arc<I>) -> Result<()> {
        let object = I::object(value, self.environment)?;
        self.environment
            .host()
            .provide(self.handle, I::INTERFACE, object)
    }

    pub fn get<I: Interface + ?Sized>(&self) -> Result<Service<I>> {
        Service::from_object(self.environment.host().get(self.handle, I::INTERFACE)?)
    }

    pub fn on<E, F>(&self, listener: F) -> Result<()>
    where
        E: SyncEvent,
        F: Fn(E) -> Result<()> + Send + Sync + 'static,
    {
        let callback = sync_callback(self.environment, move |_, _, payload| {
            listener(crate::decode(payload)?)
        })?;
        self.environment
            .host()
            .on(self.handle, E::INTERFACE, callback)
    }

    pub fn emit<E: SyncEvent>(&self, event: E) -> Result<()> {
        self.environment
            .host()
            .emit(self.handle, E::INTERFACE, &crate::encode(&event)?)
    }

    pub fn on_async<E, F, Fut>(&self, listener: F) -> Result<()>
    where
        E: AsyncEvent,
        F: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let listener = Arc::new(listener);
        let callback = async_callback(self.environment, move |_, _, payload| {
            let listener = listener.clone();
            async move { listener(crate::decode(&payload)?).await }
        })?;
        self.environment
            .host()
            .on(self.handle, E::INTERFACE, callback)
    }

    pub async fn emit_async<E: AsyncEvent>(&self, event: E) -> Result<()> {
        self.environment
            .host()
            .emit_async(self.handle, E::INTERFACE, &crate::encode(&event)?)?
            .await?;
        Ok(())
    }

    pub fn export_state<T, F>(&self, schema: &str, version: u32, exporter: F) -> Result<Frozen>
    where
        T: Serialize,
        F: Fn() -> Result<T> + Send + Sync + 'static,
    {
        let frozen = Frozen::new();
        self.export_state_with(schema, version, frozen.clone(), exporter)?;
        Ok(frozen)
    }

    pub fn export_state_with<T, F>(
        &self,
        schema: &str,
        version: u32,
        frozen: Frozen,
        exporter: F,
    ) -> Result<()>
    where
        T: Serialize,
        F: Fn() -> Result<T> + Send + Sync + 'static,
    {
        let export_flag = frozen.clone();
        let callback = state_exporter(
            self.environment,
            move || {
                export_flag.freeze();
                crate::encode(&exporter()?)
            },
            move || {
                frozen.thaw();
                Ok(())
            },
        )?;
        self.environment
            .host()
            .export_state(self.handle, schema, version, callback)
    }

    pub fn inherited_state(&self) -> Result<Option<StateBlob>> {
        Ok(self
            .environment
            .host()
            .inherited_state(self.handle)?
            .map(|state| StateBlob {
                schema: state.schema,
                version: state.version,
                data: state.data,
            }))
    }
}

/// 外部句柄由Clone共享，最后一个引用释放宿主句柄；效应本身由所属上下文管理。
#[derive(Clone)]
pub struct Effect {
    inner: Arc<EffectHandle>,
}

struct EffectHandle {
    environment: Environment,
    handle: u64,
}

impl Effect {
    fn new(environment: Environment, handle: u64) -> Self {
        Self {
            inner: Arc::new(EffectHandle {
                environment,
                handle,
            }),
        }
    }

    pub async fn settled(&self) -> Result<()> {
        self.inner
            .environment
            .host()
            .effect_settled(self.inner.handle)?
            .await?;
        Ok(())
    }

    pub async fn dispose(&self) -> Result<()> {
        self.inner
            .environment
            .host()
            .effect_dispose(self.inner.handle)?
            .await?;
        Ok(())
    }
}

impl Drop for EffectHandle {
    fn drop(&mut self) {
        if let Err(error) = self.environment.host().effect_release(self.handle) {
            self.environment.report(error);
        }
    }
}

#[derive(Clone)]
pub struct EffectScope {
    context: Context,
    handle: u64,
}

impl EffectScope {
    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn defer<F>(&self, cleanup: F) -> Result<()>
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let cleanup = Mutex::new(Some(cleanup));
        let environment = self.context.environment;
        let callback = sync_callback(environment, move |_, _, _| take_once(&cleanup)?())?;
        environment.host().defer(self.handle, callback)
    }

    pub fn defer_async<F, Fut>(&self, cleanup: F) -> Result<()>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let cleanup = Mutex::new(Some(cleanup));
        let environment = self.context.environment;
        let callback = async_callback(environment, move |_, _, _| {
            let cleanup = take_once(&cleanup);
            async move { cleanup?().await }
        })?;
        environment.host().defer_async(self.handle, callback)
    }
}

fn take_once<T>(value: &Mutex<Option<T>>) -> Result<T> {
    value
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| Error::new(4, "回调已执行"))
}
