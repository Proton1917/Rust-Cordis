#![allow(clippy::too_many_arguments)]

include!("support/interfaces.rs");
#[path = "support/host.rs"]
mod host;

mod kv_plugin {
    use kv_mem_component::KvStore;
    cordis_sdk::export_plugin!(name = "kv", version = "0.1.0", components = [KvStore]);
}

mod counter_plugin {
    use counter_component::Counter;
    cordis_sdk::export_plugin!(name = "counter", version = "0.1.0", components = [Counter]);
}

use cordis_abi::testing::{LoadedPlugin, environment};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

struct Relay(cordis_abi::Object);

impl cordis_abi::GenericService for Relay {
    fn invoke(&self, method: &str, args: &[u8]) -> Result<cordis_abi::Buf> {
        self.0.invoke(method, args)
    }

    fn invoke_async(&self, method: &str, args: &[u8]) -> Result<cordis_abi::InvokeFuture> {
        self.0.invoke_async(method, args)
    }
}

fn through_generic<I: Interface + ?Sized>(object: cordis_abi::Object) -> Result<Service<I>> {
    let name = object.interface()?.to_owned();
    let methods = object.method_names()?;
    let proxy = cordis_abi::generic_object(name, methods, Arc::new(Relay(object)), environment(3))?;
    assert!(!proxy.has_typed_methods());
    Service::<I>::from_object(proxy)
}

fn counted_service(released: Arc<AtomicUsize>) -> Result<Service<dyn Value>> {
    Service::from_object(<dyn Value as Interface>::object(
        Arc::new(CountedValue(released)),
        environment(3),
    )?)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ProbeConfig {
    value: u64,
    panic: bool,
    fail: bool,
}
struct Probe;
impl Component for Probe {
    const NAME: &'static str = "probe";
    type Config = ProbeConfig;
    fn resolve(config: &ProbeConfig) -> Result<()> {
        if config.value == 0 {
            Err(Error::new(2, "value must be positive"))
        } else {
            Ok(())
        }
    }
    async fn apply(ctx: Context, config: ProbeConfig) -> Result<()> {
        if config.panic {
            panic!("apply failure");
        }
        if config.fail {
            return Err(Error::new(1043, "apply rejected"));
        }
        let current = ctx.config::<ProbeConfig>()?;
        ctx.log(1, &format!("probe {} / {}", config.value, current.value))
    }
}
cordis_sdk::export_plugin!(name = "probe", version = "1.0.0", components = [Probe]);

struct CountedValue(Arc<AtomicUsize>);
impl Value for CountedValue {
    fn value(&self) -> Result<u64> {
        Ok(7)
    }
}
impl Drop for CountedValue {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct TaskResource(Arc<AtomicBool>);
impl Drop for TaskResource {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_loopback_covers_host_protocol_and_plugin_behavior() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), exercise())
        .await
        .map_err(|_| Error::new(6, "回环行为测试超过 10 秒"))?
}

async fn exercise() -> Result<()> {
    let host = Arc::new(host::Loopback::new());
    cordis_abi::testing::install(host.clone())?;
    let probe = LoadedPlugin::load(cordis_entry(), 3)?;
    let config = cordis_sdk::encode(&ProbeConfig {
        value: 9,
        panic: false,
        fail: false,
    })?;
    let id = host.context(config.clone(), Vec::new(), None);
    probe.resolve(0, &config)?;
    probe.apply(0, id, &config)?.await?;
    assert!(host.logs().iter().any(|line| line == "probe 9 / 9"));
    let ctx = Context::new(environment(3), id);

    let invalid = cordis_sdk::encode(&ProbeConfig {
        value: 0,
        panic: false,
        fail: false,
    })?;
    let error = probe.resolve(0, &invalid).unwrap_err();
    assert_eq!(error.code(), 2);
    #[derive(serde::Deserialize)]
    struct Issue {
        path: String,
        message: String,
    }
    let issues: Vec<Issue> = cordis_sdk::decode(error.detail())?;
    assert_eq!(issues[0].path, "");
    assert_eq!(issues[0].message, "value must be positive");
    for (panic, fail, code) in [(true, false, 5), (false, true, 1043)] {
        let config = cordis_sdk::encode(&ProbeConfig {
            value: 1,
            panic,
            fail,
        })?;
        let failed = host.context(config.clone(), Vec::new(), None);
        assert_eq!(
            probe.apply(0, failed, &config)?.await.err().unwrap().code(),
            code
        );
        host.dispose(failed).await?;
    }

    let clock = ctx.now()?;
    ctx.sleep(Duration::from_millis(1)).await?;
    assert!(ctx.now()? > clock);
    assert!(ctx.is_current()?);

    let released = Arc::new(AtomicUsize::new(0));
    ctx.provide::<dyn Value>(Arc::new(CountedValue(released.clone())))?;
    ctx.provide::<dyn Types>(Arc::new(Transformer))?;
    let typed = ctx.get::<dyn Types>()?;
    assert!(typed.clone_object()?.has_typed_methods());
    let generic = through_generic::<dyn Types>(typed.clone_object()?)?;
    for service in [typed, generic] {
        assert_eq!(
            service.numeric(true, -1, -2, -3, -4, 5, 6, 7, 8, 1.5, 2.5)?,
            20
        );
        assert!(!service.invert(true)?);
        assert_eq!(service.annotate("记录", &[1, 2, 3])?, "记录:3");
        assert_eq!(service.owned("payload".into())?, "PAYLOAD");
        service.unit()?;
        assert_eq!(service.normalize(vec![3, 1, 3, 2])?, vec![1, 2, 3]);
        assert_eq!(
            service.optional(Some("x"), Some(vec![4, 5]))?,
            Some("x:2".into())
        );
        assert_eq!(service.optional(None, Some(vec![4]))?, None);
        let revised = service.revise(Document {
            title: "note".into(),
            bytes: vec![2, 4],
            revisions: 3,
        })?;
        assert_eq!(
            revised,
            Document {
                title: "note / revised".into(),
                bytes: vec![2, 4],
                revisions: 4
            }
        );
        assert_eq!(service.service(ctx.get::<dyn Value>()?)?.value()?, 7);
        assert_eq!(
            service
                .maybe_service(Some(ctx.get::<dyn Value>()?))?
                .unwrap()
                .value()?,
            7
        );
        assert!(service.maybe_service(None)?.is_none());
        assert_eq!(
            service
                .service_async(ctx.get::<dyn Value>()?)
                .await?
                .value()?,
            7
        );
        for nullable in [None, Some(None), Some(Some(()))] {
            assert_eq!(service.nullable(nullable)?, nullable);
        }
        assert_eq!(host.resources(), 0);
        let failed_resource = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            service
                .reject_service(counted_service(failed_resource.clone())?)
                .unwrap_err()
                .code(),
            1044
        );
        assert_eq!(failed_resource.load(Ordering::SeqCst), 1);
        assert_eq!(host.resources(), 0);
        let cancelled_resource = Arc::new(AtomicUsize::new(0));
        let mut pending = service.hold_service(counted_service(cancelled_resource.clone())?);
        assert!(futures_util::poll!(pending.as_mut()).is_pending());
        assert_eq!(host.resources(), 0);
        assert_eq!(cancelled_resource.load(Ordering::SeqCst), 0);
        drop(pending);
        assert_eq!(cancelled_resource.load(Ordering::SeqCst), 1);
        assert_eq!(
            service.borrowed("async", &[8, 9]).await?,
            Document {
                title: "async".into(),
                bytes: vec![8, 9],
                revisions: 1
            }
        );
        let error = service.business_error().unwrap_err();
        assert_eq!(error.code(), 1042);
        assert_eq!(cordis_sdk::decode::<String>(error.detail())?, "locked");
        assert_eq!(service.panic().unwrap_err().code(), 5);
        assert_eq!(service.annotate("after panic", &[])?, "after panic:0");
    }
    exercise_generic_entrypoints(&ctx, &host).await?;
    {
        let provider = <dyn current::Versioned as Interface>::object(
            Arc::new(CurrentImplementation),
            environment(3),
        )?;
        assert_eq!(provider.method_names()?, vec!["value", "added"]);
        let older_generic = through_generic::<dyn old::Versioned>(provider.try_clone()?)?;
        let older = Service::<dyn old::Versioned>::from_object(provider)?;
        for older in [older, older_generic] {
            assert_eq!(older.value()?, "current");
        }
        let provider =
            <dyn old::Versioned as Interface>::object(Arc::new(OldImplementation), environment(3))?;
        assert_eq!(provider.method_names()?, vec!["value"]);
        let newer_generic = through_generic::<dyn current::Versioned>(provider.try_clone()?)?;
        let newer = Service::<dyn current::Versioned>::from_object(provider)?;
        for newer in [newer, newer_generic] {
            assert_eq!(newer.value()?, "old");
            assert_eq!(newer.added().unwrap_err().code(), 3);
        }
    }

    let old_events = Arc::new(Mutex::new(Vec::new()));
    let current_events = Arc::new(Mutex::new(Vec::new()));
    let async_events = Arc::new(Mutex::new(Vec::new()));
    ctx.on::<OldEvent, _>({
        let events = old_events.clone();
        move |event| {
            events.lock().unwrap().push(event.count);
            Ok(())
        }
    })?;
    ctx.on::<CurrentEvent, _>({
        let events = current_events.clone();
        move |event| {
            events.lock().unwrap().push((event.count, event.text));
            Ok(())
        }
    })?;
    ctx.on_async::<AsyncChanged, _, _>({
        let events = async_events.clone();
        move |event| {
            let events = events.clone();
            async move {
                tokio::task::yield_now().await;
                events.lock().unwrap().push(event.count);
                Ok(())
            }
        }
    })?;
    ctx.emit(CurrentEvent {
        count: 41,
        text: "new".into(),
    })?;
    ctx.emit(OldEvent { count: 42 })?;
    assert_eq!(*old_events.lock().unwrap(), vec![41, 42]);
    assert_eq!(
        *current_events.lock().unwrap(),
        vec![(41, "new".into()), (42, String::new())]
    );
    assert!(async_events.lock().unwrap().is_empty());
    ctx.emit_async(AsyncChanged { count: 43 }).await?;
    assert_eq!(*async_events.lock().unwrap(), vec![43]);
    assert_eq!(*old_events.lock().unwrap(), vec![41, 42]);

    let order = Arc::new(Mutex::new(Vec::new()));
    let effect = ctx.effect("ordered resources", {
        let order = order.clone();
        move |scope| {
            scope.defer({
                let order = order.clone();
                move || {
                    order.lock().unwrap().push("sync");
                    Ok(())
                }
            })?;
            scope.defer_async(move || async move {
                tokio::task::yield_now().await;
                order.lock().unwrap().push("async");
                Ok(())
            })
        }
    })?;
    effect.settled().await?;
    effect.dispose().await?;
    assert_eq!(*order.lock().unwrap(), vec!["async", "sync"]);
    drop(effect);
    let async_effect = ctx.effect_async("async body", {
        let order = order.clone();
        move |scope| async move {
            tokio::task::yield_now().await;
            scope.defer(move || {
                order.lock().unwrap().push("async body cleanup");
                Ok(())
            })
        }
    })?;
    async_effect.settled().await?;

    let task_released = Arc::new(AtomicBool::new(false));
    let (started, started_rx) = tokio::sync::oneshot::channel();
    ctx.spawn({
        let released = task_released.clone();
        async move {
            let _resource = TaskResource(released);
            started.send(()).unwrap();
            std::future::pending::<()>().await;
            Ok(())
        }
    })?;
    started_rx.await.unwrap();
    let thread_stopped = Arc::new(AtomicBool::new(false));
    let (thread_started, thread_rx) = tokio::sync::oneshot::channel();
    ctx.spawn_thread("sdk-loopback", {
        let stopped = thread_stopped.clone();
        move |ctx| {
            thread_started.send(()).unwrap();
            while ctx.is_current()? {
                std::thread::park_timeout(Duration::from_millis(1));
            }
            stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    })?;
    thread_rx.await.unwrap();

    exercise_examples(&host).await?;
    host.dispose(id).await?;
    assert!(task_released.load(Ordering::SeqCst));
    assert!(thread_stopped.load(Ordering::SeqCst));
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(
        *order.lock().unwrap(),
        vec!["async", "sync", "async body cleanup"]
    );
    assert_eq!(ctx.is_current().unwrap_err().code(), 4);
    drop(async_effect);
    drop(probe);
    assert_eq!(host.called_slots(), (1 << 26) - 1);
    assert_eq!(host.resources(), 0);
    assert_eq!(host.effect_records(), (0, 0));
    assert!(
        host.leases().values().all(|count| *count == 0),
        "{:?}",
        host.leases()
    );
    Ok(())
}

async fn exercise_generic_entrypoints(ctx: &Context, host: &host::Loopback) -> Result<()> {
    use cordis_abi::{CborEncoder, CborValue};
    let object = ctx.get::<dyn Types>()?.clone_object()?;
    let empty = cordis_sdk::encode(&CborValue::Array(Vec::new()))?;
    assert_eq!(object.invoke("missing", &empty).err().unwrap().code(), 3);
    assert_eq!(
        object.invoke_async("missing", &empty).err().unwrap().code(),
        3
    );
    assert_eq!(object.invoke("borrowed", &empty).err().unwrap().code(), 2);
    assert_eq!(object.invoke("invert", &empty).err().unwrap().code(), 2);
    let unit = object.invoke("unit", &empty)?;
    assert!(matches!(
        cordis_sdk::decode::<CborValue>(unit.as_slice())?,
        CborValue::Null
    ));
    unit.release()?;
    let arguments = cordis_sdk::encode(&CborValue::Array(vec![CborValue::Bool(true)]))?;
    let output = object.invoke_async("invert", &arguments)?.await?;
    assert!(!cordis_sdk::decode::<bool>(output.as_slice())?);
    output.release()?;

    let ignored = Arc::new(AtomicUsize::new(0));
    let mut encoder = CborEncoder::new(environment(3));
    let argument = encoder.export(<dyn Value as Interface>::object(
        Arc::new(CountedValue(ignored.clone())),
        environment(3),
    )?)?;
    let arguments = encoder.finish(CborValue::Array(vec![argument]))?;
    let output = object.invoke("service", arguments.as_slice())?;
    arguments.release()?;
    assert_eq!(host.resources(), 1);
    assert_eq!(ignored.load(Ordering::SeqCst), 0);
    output.release()?;
    assert_eq!(host.resources(), 0);
    assert_eq!(ignored.load(Ordering::SeqCst), 1);
    Ok(())
}

async fn exercise_examples(host: &host::Loopback) -> Result<()> {
    let kv_plugin = LoadedPlugin::load(kv_plugin::cordis_entry(), 1)?;
    let counter_plugin = LoadedPlugin::load(counter_plugin::cordis_entry(), 2)?;
    assert_eq!(kv_plugin.name()?, "kv");
    let config = cordis_sdk::encode(&EmptyConfig {})?;
    let store = host.context(config.clone(), Vec::new(), None);
    kv_plugin.resolve(0, &config)?;
    kv_plugin.apply(0, store, &config)?.await?;
    let kv = Context::new(environment(1), store).get::<dyn kv_api::Kv>()?;
    kv.put("greeting", b"hello".to_vec())?;
    kv.flush().await?;
    assert_eq!(kv.keys()?, vec!["greeting"]);

    #[derive(serde::Serialize)]
    struct CounterInput {
        interval_ms: u64,
        key: &'static str,
    }
    let counter_config = cordis_sdk::encode(&CounterInput {
        interval_ms: 1,
        key: "count",
    })?;
    let counter = host.context(counter_config.clone(), vec![store], None);
    counter_plugin.resolve(0, &counter_config)?;
    counter_plugin.apply(0, counter, &counter_config)?.await?;
    wait_count(&kv, 2).await?;
    let _snapshot = host.export(store)?;
    assert_eq!(kv.put("blocked", vec![1]).unwrap_err().code(), 4);
    host.thaw(store)?;
    kv.put("after-thaw", vec![2])?;
    let counter_state = host.export(counter)?;
    let before: u64 = cordis_sdk::decode(&counter_state.data)?;
    let state = host.export(store)?;
    host.dispose(counter).await?;
    drop(kv);
    host.dispose(store).await?;

    let replacement = host.context(config.clone(), Vec::new(), Some(state));
    kv_plugin.apply(0, replacement, &config)?.await?;
    let restored = Context::new(environment(1), replacement).get::<dyn kv_api::Kv>()?;
    assert_eq!(restored.get("greeting")?, Some(b"hello".to_vec()));
    assert_eq!(restored.get("after-thaw")?, Some(vec![2]));
    let restarted = host.context(
        counter_config.clone(),
        vec![replacement],
        Some(counter_state),
    );
    counter_plugin.apply(0, restarted, &counter_config)?.await?;
    wait_count(&restored, before + 1).await?;
    host.dispose(restarted).await?;
    drop(restored);
    host.dispose(replacement).await?;
    drop((kv_plugin, counter_plugin));
    Ok(())
}

async fn wait_count(kv: &Service<dyn kv_api::Kv>, expected: u64) -> Result<()> {
    loop {
        if let Some(bytes) = kv.get("count")? {
            let bytes: [u8; 8] = bytes
                .try_into()
                .map_err(|_| Error::new(2, "counter 值长度错误"))?;
            if u64::from_le_bytes(bytes) >= expected {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
