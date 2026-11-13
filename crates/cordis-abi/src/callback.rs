use crate::{
    AbiBuf, AbiBytes, AbiError, AbiFuture, AbiStatus, AbiUnit, AsyncCallbackVTable, Buf,
    Environment, ExporterVTable, Object, ObjectHeader, Result, SyncCallbackVTable, boundary,
    boundary_unit, make_future, object_arc, object_data, object_environment,
};
use std::{ffi::c_void, future::Future, mem::size_of, sync::Arc};

struct SyncCallback<F>(F);
impl<F> SyncCallback<F>
where
    F: Fn(u64, u64, &[u8]) -> Result<()> + Send + Sync + 'static,
{
    const VTABLE: SyncCallbackVTable = SyncCallbackVTable {
        header: ObjectHeader::for_type::<Self>(size_of::<SyncCallbackVTable>(), 2, ""),
        call: call_sync::<F>,
    };
}
pub fn sync_callback<F>(env: Environment, callback: F) -> Result<Object>
where
    F: Fn(u64, u64, &[u8]) -> Result<()> + Send + Sync + 'static,
{
    // SAFETY: DOC-12 §7.4，指针从完整静态虚表派生，全部槽与SyncCallback<F>配套。
    unsafe {
        Object::new(
            Arc::new(SyncCallback(callback)),
            std::ptr::from_ref(&SyncCallback::<F>::VTABLE).cast(),
            env,
        )
    }
}
// SAFETY: DOC-12 §7.4，data与F由同步回调构造函数配套。
unsafe extern "C" fn call_sync<F>(
    data: *mut c_void,
    ctx: u64,
    scope: u64,
    payload: AbiBytes,
    err: *mut AbiError,
) -> AbiStatus
where
    F: Fn(u64, u64, &[u8]) -> Result<()> + Send + Sync + 'static,
{
    // SAFETY: DOC-12 §7，data具有Object公共前缀。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §5、§7.4，闭包与借用payload在边界调用期间有效。
    unsafe {
        boundary_unit(env, err, || {
            (object_data::<SyncCallback<F>>(data).0)(ctx, scope, payload.as_slice())
        })
    }
}

struct AsyncCallback<F>(F);
impl<F, Fut> AsyncCallback<F>
where
    F: Fn(u64, u64, Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    const VTABLE: AsyncCallbackVTable = AsyncCallbackVTable {
        header: ObjectHeader::for_type::<Self>(size_of::<AsyncCallbackVTable>(), 3, ""),
        call: call_async::<F, Fut>,
    };
}
pub fn async_callback<F, Fut>(env: Environment, callback: F) -> Result<Object>
where
    F: Fn(u64, u64, Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    // SAFETY: DOC-12 §7.4，指针从完整静态虚表派生，全部槽与AsyncCallback<F>配套。
    unsafe {
        Object::new(
            Arc::new(AsyncCallback(callback)),
            std::ptr::from_ref(&AsyncCallback::<F>::VTABLE).cast(),
            env,
        )
    }
}
// SAFETY: DOC-12 §7.4，data与F由异步回调构造函数配套。
unsafe extern "C" fn call_async<F, Fut>(
    data: *mut c_void,
    ctx: u64,
    scope: u64,
    payload: AbiBytes,
    out: *mut AbiFuture,
    err: *mut AbiError,
) -> AbiStatus
where
    F: Fn(u64, u64, Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    // SAFETY: DOC-12 §7，data具有Object公共前缀。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §5、§7.4，先准备拥有型输入及Future，Future持有回调实现。
    unsafe {
        boundary(env, out, err, || {
            let callback = object_arc::<AsyncCallback<F>>(data);
            let bytes = payload.as_slice().to_vec();
            let pending = (callback.0)(ctx, scope, bytes);
            make_future(
                async move {
                    let _callback = callback;
                    pending.await?;
                    Ok(AbiUnit::default())
                },
                env,
            )
            .map(|future| future.into_raw())
        })
    }
}

struct Exporter<E, T> {
    export: E,
    thaw: T,
}
impl<E, T> Exporter<E, T>
where
    E: Fn() -> Result<Vec<u8>> + Send + Sync + 'static,
    T: Fn() -> Result<()> + Send + Sync + 'static,
{
    const VTABLE: ExporterVTable = ExporterVTable {
        header: ObjectHeader::for_type::<Self>(size_of::<ExporterVTable>(), 4, ""),
        export: export::<E, T>,
        thaw: thaw::<E, T>,
    };
}
pub fn state_exporter<E, T>(env: Environment, export: E, thaw: T) -> Result<Object>
where
    E: Fn() -> Result<Vec<u8>> + Send + Sync + 'static,
    T: Fn() -> Result<()> + Send + Sync + 'static,
{
    // SAFETY: DOC-12 §7.4，指针覆盖完整静态虚表，export和thaw均与Exporter配套。
    unsafe {
        Object::new(
            Arc::new(Exporter { export, thaw }),
            std::ptr::from_ref(&Exporter::<E, T>::VTABLE).cast(),
            env,
        )
    }
}
// SAFETY: DOC-12 §7.4，data是配套的Exporter<E,T>。
unsafe extern "C" fn export<E, T>(
    data: *mut c_void,
    out: *mut AbiBuf,
    err: *mut AbiError,
) -> AbiStatus
where
    E: Fn() -> Result<Vec<u8>> + Send + Sync + 'static,
    T: Fn() -> Result<()> + Send + Sync + 'static,
{
    // SAFETY: DOC-12 §7，data具有有效Object公共前缀。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §5、§7.4，导出结果经源侧拥有缓冲交出。
    unsafe {
        boundary(env, out, err, || {
            Buf::new((object_data::<Exporter<E, T>>(data).export)()?, env).map(Buf::into_raw)
        })
    }
}
// SAFETY: DOC-12 §7.4，data是配套的Exporter<E,T>。
unsafe extern "C" fn thaw<E, T>(data: *mut c_void, err: *mut AbiError) -> AbiStatus
where
    E: Fn() -> Result<Vec<u8>> + Send + Sync + 'static,
    T: Fn() -> Result<()> + Send + Sync + 'static,
{
    // SAFETY: DOC-12 §7，data具有有效Object公共前缀。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §6、§7.4，解除冻结在panic边界内调用。
    unsafe { boundary_unit(env, err, || (object_data::<Exporter<E, T>>(data).thaw)()) }
}
