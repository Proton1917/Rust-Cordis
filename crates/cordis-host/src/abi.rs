//! DOC-12 §9：固定 HostV1 函数表将 ABI 参数交给宿主操作并统一返回拥有错误。

use crate::{host::Host, lease};
use cordis_abi::*;
use std::sync::OnceLock;

static BACKEND: OnceLock<Host> = OnceLock::new();

pub fn install(host: Host) -> Result<()> {
    BACKEND
        .set(host)
        .map_err(|_| Error::new(4, "原生宿主已初始化"))
}

fn host() -> &'static Host {
    BACKEND.get().expect("原生宿主在装载插件前初始化")
}

pub fn environment() -> Environment {
    // SAFETY: DOC-12 §9，HOST的28个槽遵守输出、错误与拥有输入协议，表具有进程寿命。
    unsafe { Environment::new(&HOST, 0) }
}

macro_rules! host_call {
    ($name:ident($($argument:ident: $ty:ty),*) -> $output:ty, $body:block) => {
        // SAFETY: DOC-12 §5、§9，HOST按声明槽签名调用，输出与错误存储由调用方提供。
        unsafe extern "C" fn $name($($argument: $ty,)* out: *mut $output, err: *mut AbiError) -> AbiStatus {
            // SAFETY: DOC-12 §5、§6，拥有参数由body接管，边界统一处理结果和panic。
            unsafe { boundary(environment(), out, err, || $body) }
        }
    };
    ($name:ident($($argument:ident: $ty:ty),*), $body:block) => {
        // SAFETY: DOC-12 §5、§9，错误存储和借用参数覆盖整个调用。
        unsafe extern "C" fn $name($($argument: $ty,)* err: *mut AbiError) -> AbiStatus {
            // SAFETY: DOC-12 §5、§6，body接管拥有参数，所有调用都在panic边界内完成。
            unsafe { boundary_unit(environment(), err, || $body) }
        }
    };
}

host_call!(log(ctx: u64, level: u32, message: AbiStr), { host().log(ctx, level, message.as_str()?) });
host_call!(now(ctx: u64) -> u64, { host().now(ctx) });
host_call!(sleep(ctx: u64, nanos: u64) -> AbiFuture, { host().sleep(ctx, nanos).map(RemoteFuture::into_raw) });
host_call!(spawn(ctx: u64, task: AbiFuture), {
    let task = RemoteFuture::from_raw(task, environment());
    host().spawn(ctx, task)
});
host_call!(effect(ctx: u64, label: AbiStr, body: AbiObject) -> u64, {
    let body = Object::from_raw(body, environment());
    host().effect(ctx, label.as_str()?, body, false)
});
host_call!(effect_async(ctx: u64, label: AbiStr, body: AbiObject) -> u64, {
    let body = Object::from_raw(body, environment());
    host().effect(ctx, label.as_str()?, body, true)
});
host_call!(defer(scope: u64, cleanup: AbiObject), {
    let cleanup = Object::from_raw(cleanup, environment());
    host().defer(scope, cleanup, false)
});
host_call!(defer_async(scope: u64, cleanup: AbiObject), {
    let cleanup = Object::from_raw(cleanup, environment());
    host().defer(scope, cleanup, true)
});
host_call!(is_current(ctx: u64) -> u8, { host().is_current(ctx).map(u8::from) });
host_call!(provide(ctx: u64, interface: AbiStr, value: AbiObject, scoped: u8), {
    let value = Object::from_raw(value, environment());
    let scoped = match scoped { 0 => false, 1 => true, _ => return Err(Error::new(2, "scoped标志需要为0或1")) };
    host().provide(ctx, interface.as_str()?, value, scoped)
});
host_call!(get(ctx: u64, interface: AbiStr) -> AbiObject, { host().get(ctx, interface.as_str()?).map(Object::into_raw) });
host_call!(on(ctx: u64, event: AbiStr, listener: AbiObject), {
    let listener = Object::from_raw(listener, environment());
    host().on(ctx, event.as_str()?, listener)
});
host_call!(emit(ctx: u64, event: AbiStr, payload: AbiBytes), { host().emit(ctx, event.as_str()?, payload.as_slice(), false) });
host_call!(emit_async(ctx: u64, event: AbiStr, payload: AbiBytes) -> AbiFuture, {
    host().emit_async(ctx, event.as_str()?, payload.as_slice()).map(RemoteFuture::into_raw)
});
host_call!(export_state(ctx: u64, schema: AbiStr, version: u32, exporter: AbiObject), {
    let exporter = Object::from_raw(exporter, environment());
    host().export_state(ctx, schema.as_str()?, version, exporter)
});
host_call!(inherited_state(ctx: u64) -> AbiOption<AbiState>, {
    match host().inherited_state(ctx)? {
        None => Ok(AbiOption::none()),
        Some(state) => {
            let schema = Buf::new(state.schema.into_bytes(), environment())?;
            let data = Buf::new(state.data, environment())?;
            Ok(AbiOption::some(AbiState { schema: schema.into_raw(), version: state.version, reserved: 0, data: data.into_raw() }))
        }
    }
});
host_call!(config(ctx: u64) -> AbiBuf, { Buf::new(host().config(ctx)?, environment()).map(Buf::into_raw) });
host_call!(spawn_thread(ctx: u64, name: AbiStr, body: AbiObject), {
    let body = Object::from_raw(body, environment());
    host().spawn_thread(ctx, name.as_str()?, body)
});
host_call!(lease_acquire(library: u64), { lease::acquire(library) });
host_call!(lease_release(library: u64), { lease::release(library) });
host_call!(effect_settled(effect: u64) -> AbiFuture, { host().effect_wait(effect, false).map(RemoteFuture::into_raw) });
host_call!(effect_dispose(effect: u64) -> AbiFuture, { host().effect_wait(effect, true).map(RemoteFuture::into_raw) });
host_call!(object_export(object: AbiObject) -> u64, {
    let object = Object::from_raw(object, environment());
    Ok(host().objects().insert(object))
});
host_call!(object_import(object: u64) -> AbiObject, { host().objects().import(object).map(Object::into_raw) });
host_call!(object_release(object: u64), { host().objects().release(object) });
host_call!(effect_release(effect: u64), { host().effect_release(effect) });
host_call!(context_derive(ctx: u64, options: AbiBytes) -> AbiContext, {
    let derived = host().context_derive(ctx, options.as_slice())?;
    Ok(AbiContext { context: derived.context, lease: derived.lease.into_raw() })
});
host_call!(plugin(ctx: u64, component: AbiStr, config: AbiBytes) -> AbiFuture, {
    host().plugin(ctx, component.as_str()?, config.as_slice()).map(RemoteFuture::into_raw)
});

static HOST: HostV1 = HostV1 {
    size: std::mem::size_of::<HostV1>() as u64,
    log,
    now,
    sleep,
    spawn,
    effect,
    effect_async,
    defer,
    defer_async,
    is_current,
    provide,
    get,
    on,
    emit,
    emit_async,
    export_state,
    inherited_state,
    config,
    spawn_thread,
    lease_acquire,
    lease_release,
    effect_settled,
    effect_dispose,
    object_export,
    object_import,
    object_release,
    effect_release,
    context_derive,
    plugin,
};
