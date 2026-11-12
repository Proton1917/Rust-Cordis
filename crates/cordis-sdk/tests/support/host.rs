use cordis_abi::testing::{Backend, environment};
use cordis_abi::{AbiUnit, Error, Object, RemoteFuture, Result, State, make_future};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

type Outcome = watch::Sender<Option<Result<()>>>;

struct Instance {
    current: AtomicBool,
    alive: AtomicBool,
    config: Vec<u8>,
    inherited: Option<State>,
    inject: Vec<u64>,
    services: Mutex<BTreeMap<String, Arc<Object>>>,
    listeners: Mutex<Vec<(String, u32, Arc<Object>)>>,
    exporter: Mutex<Option<(String, u32, Arc<Object>)>>,
    effects: Mutex<Vec<(u64, Arc<Effect>)>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<Result<AbiUnit>>>>,
    threads: Mutex<Vec<std::thread::JoinHandle<Result<()>>>>,
}

struct Effect {
    context: u64,
    cleanups: Mutex<Vec<(bool, Object)>>,
    running: Outcome,
    disposed: AtomicBool,
    cleaned: Outcome,
}

struct Inner {
    instances: Mutex<BTreeMap<u64, Arc<Instance>>>,
    effects: Mutex<BTreeMap<u64, Arc<Effect>>>,
    effect_handles: Mutex<BTreeMap<u64, Arc<Effect>>>,
    next: AtomicU64,
    leases: Mutex<BTreeMap<u64, usize>>,
    resources: Mutex<BTreeMap<u64, Arc<Object>>>,
    calls: AtomicU32,
    logs: Mutex<Vec<String>>,
    started: Instant,
}

#[derive(Clone)]
pub struct Loopback {
    inner: Arc<Inner>,
    runtime: tokio::runtime::Handle,
}

impl Default for Loopback {
    fn default() -> Self {
        Self::new()
    }
}

impl Loopback {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                instances: Mutex::new(BTreeMap::new()),
                effects: Mutex::new(BTreeMap::new()),
                effect_handles: Mutex::new(BTreeMap::new()),
                next: AtomicU64::new(1),
                leases: Mutex::new(BTreeMap::new()),
                resources: Mutex::new(BTreeMap::new()),
                calls: AtomicU32::new(0),
                logs: Mutex::new(Vec::new()),
                started: Instant::now(),
            }),
            runtime: tokio::runtime::Handle::current(),
        }
    }

    fn record(&self, slot: u32) {
        self.inner.calls.fetch_or(1 << slot, Ordering::Relaxed);
    }
    pub fn called_slots(&self) -> u32 {
        self.inner.calls.load(Ordering::Relaxed)
    }
    pub fn logs(&self) -> Vec<String> {
        self.inner.logs.lock().unwrap().clone()
    }
    pub fn leases(&self) -> BTreeMap<u64, usize> {
        self.inner.leases.lock().unwrap().clone()
    }

    pub fn resources(&self) -> usize {
        self.inner.resources.lock().unwrap().len()
    }

    pub fn effect_records(&self) -> (usize, usize) {
        (
            self.inner.effects.lock().unwrap().len(),
            self.inner.effect_handles.lock().unwrap().len(),
        )
    }

    pub fn context(&self, config: Vec<u8>, inject: Vec<u64>, inherited: Option<State>) -> u64 {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        self.inner.instances.lock().unwrap().insert(
            id,
            Arc::new(Instance {
                current: AtomicBool::new(true),
                alive: AtomicBool::new(true),
                config,
                inherited,
                inject,
                services: Mutex::new(BTreeMap::new()),
                listeners: Mutex::new(Vec::new()),
                exporter: Mutex::new(None),
                effects: Mutex::new(Vec::new()),
                tasks: Mutex::new(Vec::new()),
                threads: Mutex::new(Vec::new()),
            }),
        );
        id
    }

    fn instance(&self, ctx: u64) -> Result<Arc<Instance>> {
        self.inner
            .instances
            .lock()
            .unwrap()
            .get(&ctx)
            .filter(|instance| instance.alive.load(Ordering::Acquire))
            .cloned()
            .ok_or_else(|| Error::new(4, "上下文已释放"))
    }

    fn effect_state(&self, id: u64) -> Result<Arc<Effect>> {
        self.inner
            .effects
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::new(4, "效应已释放"))
    }

    fn effect_handle(&self, id: u64) -> Result<Arc<Effect>> {
        self.inner
            .effect_handles
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::new(4, "效应句柄已释放"))
    }

    fn release_effect_handle(&self, id: u64) -> Result<()> {
        let effect = self
            .inner
            .effect_handles
            .lock()
            .unwrap()
            .remove(&id)
            .ok_or_else(|| Error::new(4, "效应句柄已释放"))?;
        drop(effect);
        Ok(())
    }

    fn create_effect(&self, ctx: u64) -> Result<(u64, Arc<Effect>)> {
        let instance = self.instance(ctx)?;
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        let effect = Arc::new(Effect {
            context: ctx,
            cleanups: Mutex::new(Vec::new()),
            running: watch::channel(None).0,
            disposed: AtomicBool::new(false),
            cleaned: watch::channel(None).0,
        });
        self.inner
            .effects
            .lock()
            .unwrap()
            .insert(id, effect.clone());
        self.inner
            .effect_handles
            .lock()
            .unwrap()
            .insert(id, effect.clone());
        instance.effects.lock().unwrap().push((id, effect.clone()));
        Ok((id, effect))
    }

    async fn cleanup(&self, id: u64, effect: Arc<Effect>) -> Result<()> {
        if effect.disposed.swap(true, Ordering::AcqRel) {
            return wait(&effect.cleaned).await;
        }
        let _execution = wait(&effect.running).await;
        let cleanups = std::mem::take(&mut *effect.cleanups.lock().unwrap());
        let mut failure = None;
        for (asynchronous, cleanup) in cleanups.into_iter().rev() {
            let result = if asynchronous {
                match cleanup.call_async(effect.context, id, &[]) {
                    Ok(future) => future.await.map(|_| ()),
                    Err(error) => Err(error),
                }
            } else {
                cleanup.call_sync(effect.context, id, &[])
            };
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        let result = failure.map_or(Ok(()), Err);
        self.instance(effect.context)?
            .effects
            .lock()
            .unwrap()
            .retain(|(effect_id, _)| *effect_id != id);
        let registration = self.inner.effects.lock().unwrap().remove(&id);
        drop(registration);
        effect.cleaned.send_replace(Some(result.clone()));
        result
    }

    pub async fn dispose(&self, ctx: u64) -> Result<()> {
        let instance = self.instance(ctx)?;
        instance.current.store(false, Ordering::Release);
        let mut failure = None;
        let tasks = std::mem::take(&mut *instance.tasks.lock().unwrap());
        for task in tasks {
            task.abort();
            match task.await {
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Ok(Ok(_)) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    failure.get_or_insert_with(|| Error::new(5, error.to_string()));
                }
            }
        }
        let effects = instance.effects.lock().unwrap().clone();
        for (id, effect) in effects.into_iter().rev() {
            if let Err(error) = self.cleanup(id, effect).await {
                failure.get_or_insert(error);
            }
        }
        let threads = std::mem::take(&mut *instance.threads.lock().unwrap());
        for thread in threads {
            match thread.join() {
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert_with(|| Error::new(5, "系统线程发生 panic"));
                }
                Ok(Ok(())) => {}
            }
        }
        let services = std::mem::take(&mut *instance.services.lock().unwrap());
        let listeners = std::mem::take(&mut *instance.listeners.lock().unwrap());
        let exporter = instance.exporter.lock().unwrap().take();
        drop((services, listeners, exporter));
        instance.alive.store(false, Ordering::Release);
        failure.map_or(Ok(()), Err)
    }

    pub fn export(&self, ctx: u64) -> Result<State> {
        let instance = self.instance(ctx)?;
        let (schema, version, exporter) = instance
            .exporter
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::new(1, "组件尚无状态导出器"))?;
        Ok(State {
            schema,
            version,
            data: exporter.export()?.into_vec()?,
        })
    }

    pub fn thaw(&self, ctx: u64) -> Result<()> {
        let instance = self.instance(ctx)?;
        let exporter = instance
            .exporter
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, _, exporter)| exporter.clone())
            .ok_or_else(|| Error::new(1, "组件尚无状态导出器"))?;
        exporter.thaw()
    }

    fn listeners(&self, event: &str, kind: u32) -> Result<Vec<Arc<Object>>> {
        let (event, _) = version(event)?;
        let instances: Vec<_> = self
            .inner
            .instances
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        let mut listeners = Vec::new();
        for instance in instances {
            if instance.alive.load(Ordering::Acquire) {
                for (key, family, callback) in instance.listeners.lock().unwrap().iter() {
                    if key == event && *family == kind {
                        listeners.push(callback.clone());
                    }
                }
            }
        }
        Ok(listeners)
    }
}

async fn wait(sender: &Outcome) -> Result<()> {
    let mut receiver = sender.subscribe();
    loop {
        if let Some(result) = receiver.borrow_and_update().clone() {
            return result;
        }
        receiver
            .changed()
            .await
            .map_err(|error| Error::new(4, error.to_string()))?;
    }
}

fn version(interface: &str) -> Result<(&str, u32)> {
    let (key, minor) = interface
        .rsplit_once('.')
        .ok_or_else(|| Error::new(2, "接口标识缺少次版本"))?;
    Ok((
        key,
        minor
            .parse()
            .map_err(|_| Error::new(2, "次版本需要为 u32"))?,
    ))
}

impl Backend for Loopback {
    fn log(&self, ctx: u64, _level: u32, message: &str) -> Result<()> {
        self.record(0);
        if ctx != 0 {
            self.instance(ctx)?;
        }
        self.inner.logs.lock().unwrap().push(message.to_owned());
        Ok(())
    }
    fn now(&self, ctx: u64) -> Result<u64> {
        self.record(1);
        self.instance(ctx)?;
        Ok(self.inner.started.elapsed().as_nanos() as u64)
    }
    fn sleep(&self, ctx: u64, nanos: u64) -> Result<RemoteFuture<AbiUnit>> {
        self.record(2);
        self.instance(ctx)?;
        make_future(
            async move {
                tokio::time::sleep(Duration::from_nanos(nanos)).await;
                Ok(AbiUnit::default())
            },
            environment(0),
        )
    }
    fn spawn(&self, ctx: u64, task: RemoteFuture<AbiUnit>) -> Result<()> {
        self.record(3);
        self.instance(ctx)?
            .tasks
            .lock()
            .unwrap()
            .push(self.runtime.spawn(task));
        Ok(())
    }
    fn effect(&self, ctx: u64, _label: &str, body: Object) -> Result<u64> {
        self.record(4);
        let (id, effect) = self.create_effect(ctx)?;
        let result = body.call_sync(ctx, id, &[]);
        effect.running.send_replace(Some(result.clone()));
        if result.is_err() {
            self.release_effect_handle(id)?;
            let host = self.clone();
            self.runtime
                .spawn(async move { host.cleanup(id, effect).await });
        }
        result.map(|()| id)
    }
    fn effect_async(&self, ctx: u64, _label: &str, body: Object) -> Result<u64> {
        self.record(5);
        let (id, effect) = self.create_effect(ctx)?;
        let host = self.clone();
        self.runtime.spawn(async move {
            let result = match body.call_async(ctx, id, &[]) {
                Ok(future) => future.await.map(|_| ()),
                Err(error) => Err(error),
            };
            drop(body);
            effect.running.send_replace(Some(result.clone()));
            if result.is_err() {
                host.cleanup(id, effect).await?;
            }
            result
        });
        Ok(id)
    }
    fn defer(&self, scope: u64, cleanup: Object) -> Result<()> {
        self.record(6);
        let effect = self.effect_state(scope)?;
        if effect.running.borrow().is_some() {
            return Err(Error::new(4, "效应体已结束"));
        }
        effect.cleanups.lock().unwrap().push((false, cleanup));
        Ok(())
    }
    fn defer_async(&self, scope: u64, cleanup: Object) -> Result<()> {
        self.record(7);
        let effect = self.effect_state(scope)?;
        if effect.running.borrow().is_some() {
            return Err(Error::new(4, "效应体已结束"));
        }
        effect.cleanups.lock().unwrap().push((true, cleanup));
        Ok(())
    }
    fn is_current(&self, ctx: u64) -> Result<bool> {
        self.record(8);
        Ok(self.instance(ctx)?.current.load(Ordering::Acquire))
    }
    fn provide(&self, ctx: u64, interface: &str, value: Object, scoped: bool) -> Result<()> {
        self.record(9);
        if scoped {
            return Err(Error::new(3, "上下文绑定能力缺席"));
        }
        let instance = self.instance(ctx)?;
        let (key, _) = version(interface)?;
        let mut services = instance.services.lock().unwrap();
        if services.contains_key(key) {
            return Err(Error::new(1, "接口已提供"));
        }
        services.insert(key.into(), Arc::new(value));
        Ok(())
    }
    fn get(&self, ctx: u64, interface: &str) -> Result<Object> {
        self.record(10);
        let instance = self.instance(ctx)?;
        let (key, required) = version(interface)?;
        for source in std::iter::once(ctx).chain(instance.inject.iter().copied()) {
            let provider = self.instance(source)?;
            let value = provider.services.lock().unwrap().get(key).cloned();
            if let Some(value) = value {
                if version(value.interface()?)?.1 < required {
                    return Err(Error::new(3, "接口次版本不足"));
                }
                return value.try_clone();
            }
        }
        Err(Error::new(4, "所需接口当前不可用"))
    }
    fn on(&self, ctx: u64, event: &str, listener: Object) -> Result<()> {
        self.record(11);
        let instance = self.instance(ctx)?;
        let (key, _) = version(event)?;
        let kind = listener.header().kind;
        if kind != 2 && kind != 3 {
            return Err(Error::new(2, "事件回调类型无效"));
        }
        instance
            .listeners
            .lock()
            .unwrap()
            .push((key.into(), kind, Arc::new(listener)));
        Ok(())
    }
    fn emit(&self, ctx: u64, event: &str, payload: &[u8]) -> Result<()> {
        self.record(12);
        self.instance(ctx)?;
        let mut failure = None;
        for listener in self.listeners(event, 2)? {
            if let Err(error) = listener.call_sync(ctx, 0, payload) {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
    fn emit_async(&self, ctx: u64, event: &str, payload: &[u8]) -> Result<RemoteFuture<AbiUnit>> {
        self.record(13);
        self.instance(ctx)?;
        let listeners = self.listeners(event, 3)?;
        let payload = payload.to_owned();
        make_future(
            async move {
                let results =
                    futures_util::future::join_all(listeners.iter().map(|listener| async {
                        listener.call_async(ctx, 0, &payload)?.await.map(|_| ())
                    }))
                    .await;
                for result in results {
                    result?;
                }
                Ok(AbiUnit::default())
            },
            environment(0),
        )
    }
    fn export_state(&self, ctx: u64, schema: &str, version: u32, exporter: Object) -> Result<()> {
        self.record(14);
        let instance = self.instance(ctx)?;
        let mut stored = instance.exporter.lock().unwrap();
        if stored.is_some() {
            return Err(Error::new(1, "状态导出器已登记"));
        }
        *stored = Some((schema.into(), version, Arc::new(exporter)));
        Ok(())
    }
    fn inherited_state(&self, ctx: u64) -> Result<Option<State>> {
        self.record(15);
        Ok(self.instance(ctx)?.inherited.clone())
    }
    fn config(&self, ctx: u64) -> Result<Vec<u8>> {
        self.record(16);
        Ok(self.instance(ctx)?.config.clone())
    }
    fn spawn_thread(&self, ctx: u64, name: &str, body: Object) -> Result<()> {
        self.record(17);
        let instance = self.instance(ctx)?;
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || body.call_sync(ctx, 0, &[]))
            .map_err(|error| Error::new(1, error.to_string()))?;
        instance.threads.lock().unwrap().push(thread);
        Ok(())
    }
    fn lease_acquire(&self, library: u64) -> Result<()> {
        self.record(18);
        *self
            .inner
            .leases
            .lock()
            .unwrap()
            .entry(library)
            .or_default() += 1;
        Ok(())
    }
    fn lease_release(&self, library: u64) -> Result<()> {
        self.record(19);
        let mut leases = self.inner.leases.lock().unwrap();
        let count = leases.entry(library).or_default();
        *count = count
            .checked_sub(1)
            .ok_or_else(|| Error::new(1, "库租约重复释放"))?;
        Ok(())
    }
    fn effect_settled(&self, id: u64) -> Result<RemoteFuture<AbiUnit>> {
        self.record(20);
        let effect = self.effect_handle(id)?;
        make_future(
            async move {
                let result = wait(&effect.running).await;
                if result.is_err() {
                    wait(&effect.cleaned).await?;
                }
                result?;
                Ok(AbiUnit::default())
            },
            environment(0),
        )
    }
    fn effect_dispose(&self, id: u64) -> Result<RemoteFuture<AbiUnit>> {
        self.record(21);
        let effect = self.effect_handle(id)?;
        let host = self.clone();
        make_future(
            async move {
                host.cleanup(id, effect).await?;
                Ok(AbiUnit::default())
            },
            environment(0),
        )
    }

    fn object_export(&self, object: Object) -> Result<u64> {
        self.record(22);
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        self.inner
            .resources
            .lock()
            .unwrap()
            .insert(id, Arc::new(object));
        Ok(id)
    }

    fn object_import(&self, id: u64) -> Result<Object> {
        self.record(23);
        let object = self
            .inner
            .resources
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::new(4, "服务资源编号已释放"))?;
        object.try_clone()
    }

    fn object_release(&self, id: u64) -> Result<()> {
        self.record(24);
        let object = self
            .inner
            .resources
            .lock()
            .unwrap()
            .remove(&id)
            .ok_or_else(|| Error::new(4, "服务资源编号已释放"))?;
        drop(object);
        Ok(())
    }

    fn effect_release(&self, id: u64) -> Result<()> {
        self.record(25);
        self.release_effect_handle(id)
    }

    fn context_derive(&self, context: u64, _options: &[u8]) -> Result<cordis_abi::DerivedContext> {
        self.instance(context)?;
        Err(Error::new(3, "上下文派生能力缺席"))
    }

    fn plugin(&self, context: u64, _component: &str, _config: &[u8]) -> Result<RemoteFuture<u64>> {
        self.instance(context)?;
        Err(Error::new(3, "组件目录缺席"))
    }
}
