//! ADR-0039、DOC-12 §7.4：事件资源在输入缓冲释放后由异步监听器持有。

#[path = "support/host.rs"]
pub mod host;
#[path = "support/resources.rs"]
mod resources;

use cordis_abi::testing::environment;
use cordis_abi::{AbiUnit, CborEncoder, RemoteFuture};
use cordis_sdk::{Context, Event, Result, Service};
use resources::{ResourceCounter, tracked};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Semaphore;

#[cordis_sdk::event(name = "test.resource-sync", version = "1.0")]
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncResource {
    service: Service<dyn ResourceCounter>,
}

#[cordis_sdk::event(name = "test.resource-async", version = "1.0", kind = "async")]
#[derive(serde::Serialize, serde::Deserialize)]
struct AsyncResource {
    service: Service<dyn ResourceCounter>,
    amount: u64,
    wait: bool,
}

#[tokio::test]
async fn events_hold_resources_until_listener_completion_or_cancellation() -> Result<()> {
    let host = Arc::new(host::Loopback::new());
    cordis_abi::testing::install(host.clone())?;
    let id = host.context(cordis_sdk::encode(&())?, Vec::new(), None);
    let source = Context::new(environment(41), id);
    let listener = Context::new(environment(42), id);
    synchronous_event_calls_service(&host, &source, &listener)?;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(Semaphore::new(0));
    listener.on_async::<AsyncResource, _, _>({
        let observed = observed.clone();
        let release = release.clone();
        move |event| {
            let observed = observed.clone();
            let release = release.clone();
            async move {
                observed
                    .lock()
                    .unwrap()
                    .push(event.service.add(event.amount)?);
                if event.wait {
                    release.acquire().await.expect("测试释放信号有效").forget();
                }
                observed.lock().unwrap().push(event.service.add(1)?);
                Ok(())
            }
        }
    })?;
    completed_listener_releases_resource(&host, &source, id, &observed, &release).await?;
    cancelled_listener_releases_resource(&host, &source, id, &observed).await?;
    sdk_emission_calls_resource(&host, &source, &observed).await?;
    host.dispose(id).await?;
    assert!(host.leases().values().all(|count| *count == 0));
    Ok(())
}

/// ADR-0039：同步事件使用 SDK 编码、实际回调和服务方法表，返回时释放引用。
fn synchronous_event_calls_service(
    host: &host::Loopback,
    source: &Context,
    listener: &Context,
) -> Result<()> {
    let observed = Arc::new(Mutex::new(Vec::new()));
    listener.on::<SyncResource, _>({
        let observed = observed.clone();
        move |event| {
            observed.lock().unwrap().push(event.service.add(4)?);
            Ok(())
        }
    })?;
    let released = Arc::new(AtomicUsize::new(0));
    source.emit(SyncResource {
        service: tracked(source, 10, released.clone())?,
    })?;
    assert_eq!(*observed.lock().unwrap(), [14]);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(host.resources(), 0);
    assert_eq!(host.leases().get(&41), Some(&0));
    Ok(())
}

/// DOC-12 §7.4：返回 Future 前释放发送侧载荷和服务，后续调用由导入引用支撑。
fn dispatch_then_release_input(
    context: u64,
    event: AsyncResource,
) -> Result<RemoteFuture<AbiUnit>> {
    let environment = environment(43);
    let buffer = CborEncoder::new(environment).encode(&event)?;
    let pending =
        environment
            .host()
            .emit_async(context, AsyncResource::INTERFACE, buffer.as_slice())?;
    buffer.release()?;
    Ok(pending)
}

/// ADR-0039：原输入已经释放，监听器恢复执行后仍可读取并修改资源。
async fn completed_listener_releases_resource(
    host: &host::Loopback,
    source: &Context,
    id: u64,
    observed: &Mutex<Vec<u64>>,
    release: &Semaphore,
) -> Result<()> {
    let released = Arc::new(AtomicUsize::new(0));
    let mut pending = dispatch_then_release_input(
        id,
        AsyncResource {
            service: tracked(source, 40, released.clone())?,
            amount: 5,
            wait: true,
        },
    )?;
    assert_eq!(host.resources(), 0);
    assert!(observed.lock().unwrap().is_empty());
    assert_eq!(released.load(Ordering::SeqCst), 0);
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert_eq!(*observed.lock().unwrap(), [45]);
    release.add_permits(1);
    pending.await?;
    assert_eq!(*observed.lock().unwrap(), [45, 46]);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(host.leases().get(&41), Some(&0));
    Ok(())
}

/// ADR-0039：取消已暂停的监听器时释放其导入对象与库租约。
async fn cancelled_listener_releases_resource(
    host: &host::Loopback,
    source: &Context,
    id: u64,
    observed: &Mutex<Vec<u64>>,
) -> Result<()> {
    let released = Arc::new(AtomicUsize::new(0));
    let mut pending = dispatch_then_release_input(
        id,
        AsyncResource {
            service: tracked(source, 80, released.clone())?,
            amount: 7,
            wait: true,
        },
    )?;
    assert_eq!(host.resources(), 0);
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert_eq!(*observed.lock().unwrap(), [45, 46, 87]);
    assert_eq!(released.load(Ordering::SeqCst), 0);
    drop(pending);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(host.leases().get(&41), Some(&0));
    Ok(())
}

/// ADR-0039：SDK 异步发送入口使用同一资源事件契约并回收编码缓冲。
async fn sdk_emission_calls_resource(
    host: &host::Loopback,
    source: &Context,
    observed: &Mutex<Vec<u64>>,
) -> Result<()> {
    let released = Arc::new(AtomicUsize::new(0));
    source
        .emit_async(AsyncResource {
            service: tracked(source, 100, released.clone())?,
            amount: 9,
            wait: false,
        })
        .await?;
    assert_eq!(*observed.lock().unwrap(), [45, 46, 87, 109, 110]);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(host.resources(), 0);
    assert_eq!(host.leases().get(&41), Some(&0));
    Ok(())
}
