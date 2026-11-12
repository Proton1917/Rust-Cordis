//! M9 回环测试的 C ABI 桥。测试后端负责实际上下文和资源操作。

use crate::*;
use std::sync::{Arc, OnceLock};

pub trait Backend: Send + Sync + 'static {
    fn log(&self, ctx: u64, level: u32, message: &str) -> Result<()>;
    fn now(&self, ctx: u64) -> Result<u64>;
    fn sleep(&self, ctx: u64, nanos: u64) -> Result<RemoteFuture<AbiUnit>>;
    fn spawn(&self, ctx: u64, task: RemoteFuture<AbiUnit>) -> Result<()>;
    fn effect(&self, ctx: u64, label: &str, body: Object) -> Result<u64>;
    fn effect_async(&self, ctx: u64, label: &str, body: Object) -> Result<u64>;
    fn defer(&self, scope: u64, cleanup: Object) -> Result<()>;
    fn defer_async(&self, scope: u64, cleanup: Object) -> Result<()>;
    fn is_current(&self, ctx: u64) -> Result<bool>;
    fn provide(&self, ctx: u64, interface: &str, value: Object) -> Result<()>;
    fn get(&self, ctx: u64, interface: &str) -> Result<Object>;
    fn on(&self, ctx: u64, event: &str, listener: Object) -> Result<()>;
    fn emit(&self, ctx: u64, event: &str, payload: &[u8]) -> Result<()>;
    fn emit_async(&self, ctx: u64, event: &str, payload: &[u8]) -> Result<RemoteFuture<AbiUnit>>;
    fn export_state(&self, ctx: u64, schema: &str, version: u32, exporter: Object) -> Result<()>;
    fn inherited_state(&self, ctx: u64) -> Result<Option<State>>;
    fn config(&self, ctx: u64) -> Result<Vec<u8>>;
    fn spawn_thread(&self, ctx: u64, name: &str, body: Object) -> Result<()>;
    fn lease_acquire(&self, library: LibraryId) -> Result<()>;
    fn lease_release(&self, library: LibraryId) -> Result<()>;
    fn effect_settled(&self, effect: u64) -> Result<RemoteFuture<AbiUnit>>;
    fn effect_dispose(&self, effect: u64) -> Result<RemoteFuture<AbiUnit>>;
    fn object_export(&self, object: Object) -> Result<u64>;
    fn object_import(&self, id: u64) -> Result<Object>;
    fn object_release(&self, id: u64) -> Result<()>;
    fn effect_release(&self, effect: u64) -> Result<()>;
}

static BACKEND: OnceLock<Arc<dyn Backend>> = OnceLock::new();

pub fn install(backend: Arc<dyn Backend>) -> Result<()> {
    BACKEND
        .set(backend)
        .map_err(|_| Error::new(4, "回环宿主已经初始化"))
}

fn backend() -> &'static dyn Backend {
    BACKEND.get().expect("回环宿主在握手前初始化").as_ref()
}

pub fn environment(library: LibraryId) -> Environment {
    // SAFETY: DOC-12 §9，本模块所有槽完整实现输出、错误和所有权协议，HOST具有进程寿命。
    unsafe { Environment::new(&HOST, library) }
}

macro_rules! host_call {
    ($name:ident($($argument:ident: $ty:ty),*) -> $output:ty, $body:block) => {
        // SAFETY: DOC-12 §5、§9，HOST按声明的槽签名调用，输出存储由调用方提供。
        unsafe extern "C" fn $name($($argument: $ty,)* out: *mut $output, err: *mut AbiError) -> AbiStatus {
            // SAFETY: DOC-12 §5、§6，输出和错误槽覆盖整个调用，body在panic边界内执行。
            unsafe { boundary(environment(0), out, err, || $body) }
        }
    };
    ($name:ident($($argument:ident: $ty:ty),*), $body:block) => {
        // SAFETY: DOC-12 §5、§9，HOST按声明的槽签名调用，错误存储由调用方提供。
        unsafe extern "C" fn $name($($argument: $ty,)* err: *mut AbiError) -> AbiStatus {
            // SAFETY: DOC-12 §5、§6，错误槽有效，body接管拥有参数并捕获panic。
            unsafe { boundary_unit(environment(0), err, || $body) }
        }
    };
}

host_call!(log(ctx: u64, level: u32, message: AbiStr), { backend().log(ctx, level, message.as_str()?) });
host_call!(now(ctx: u64) -> u64, { backend().now(ctx) });
host_call!(sleep(ctx: u64, nanos: u64) -> AbiFuture, { backend().sleep(ctx, nanos).map(RemoteFuture::into_raw) });
host_call!(spawn(ctx: u64, task: AbiFuture), {
    let task = RemoteFuture::from_raw(task, environment(0));
    backend().spawn(ctx, task)
});
host_call!(effect(ctx: u64, label: AbiStr, body: AbiObject) -> u64, {
    let body = Object::from_raw(body, environment(0));
    backend().effect(ctx, label.as_str()?, body)
});
host_call!(effect_async(ctx: u64, label: AbiStr, body: AbiObject) -> u64, {
    let body = Object::from_raw(body, environment(0));
    backend().effect_async(ctx, label.as_str()?, body)
});
host_call!(defer(scope: u64, cleanup: AbiObject), {
    let cleanup = Object::from_raw(cleanup, environment(0));
    backend().defer(scope, cleanup)
});
host_call!(defer_async(scope: u64, cleanup: AbiObject), {
    let cleanup = Object::from_raw(cleanup, environment(0));
    backend().defer_async(scope, cleanup)
});
host_call!(is_current(ctx: u64) -> u8, { backend().is_current(ctx).map(u8::from) });
host_call!(provide(ctx: u64, interface: AbiStr, value: AbiObject), {
    let value = Object::from_raw(value, environment(0));
    backend().provide(ctx, interface.as_str()?, value)
});
host_call!(get(ctx: u64, interface: AbiStr) -> AbiObject, { backend().get(ctx, interface.as_str()?).map(Object::into_raw) });
host_call!(on(ctx: u64, event: AbiStr, listener: AbiObject), {
    let listener = Object::from_raw(listener, environment(0));
    backend().on(ctx, event.as_str()?, listener)
});
host_call!(emit(ctx: u64, event: AbiStr, payload: AbiBytes), { backend().emit(ctx, event.as_str()?, payload.as_slice()) });
host_call!(emit_async(ctx: u64, event: AbiStr, payload: AbiBytes) -> AbiFuture, {
    backend().emit_async(ctx, event.as_str()?, payload.as_slice()).map(RemoteFuture::into_raw)
});
host_call!(export_state(ctx: u64, schema: AbiStr, version: u32, exporter: AbiObject), {
    let exporter = Object::from_raw(exporter, environment(0));
    backend().export_state(ctx, schema.as_str()?, version, exporter)
});
host_call!(inherited_state(ctx: u64) -> AbiOption<AbiState>, {
    match backend().inherited_state(ctx)? {
        None => Ok(AbiOption::none()),
        Some(state) => {
            let schema = Buf::new(state.schema.into_bytes(), environment(0))?;
            let data = Buf::new(state.data, environment(0))?;
            Ok(AbiOption::some(AbiState { schema: schema.into_raw(), version: state.version,
                reserved: 0, data: data.into_raw() }))
        }
    }
});
host_call!(config(ctx: u64) -> AbiBuf, { Buf::new(backend().config(ctx)?, environment(0)).map(Buf::into_raw) });
host_call!(spawn_thread(ctx: u64, name: AbiStr, body: AbiObject), {
    let body = Object::from_raw(body, environment(0));
    backend().spawn_thread(ctx, name.as_str()?, body)
});
host_call!(lease_acquire(library: LibraryId), { backend().lease_acquire(library) });
host_call!(lease_release(library: LibraryId), { backend().lease_release(library) });
host_call!(effect_settled(effect: u64) -> AbiFuture, { backend().effect_settled(effect).map(RemoteFuture::into_raw) });
host_call!(effect_dispose(effect: u64) -> AbiFuture, { backend().effect_dispose(effect).map(RemoteFuture::into_raw) });
host_call!(object_export(object: AbiObject) -> u64, {
    let object = Object::from_raw(object, environment(0));
    backend().object_export(object)
});
host_call!(object_import(id: u64) -> AbiObject, { backend().object_import(id).map(Object::into_raw) });
host_call!(object_release(id: u64), { backend().object_release(id) });
host_call!(effect_release(effect: u64), { backend().effect_release(effect) });

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
};

pub struct LoadedPlugin {
    descriptor: *const PluginV1,
    library: LibraryId,
    _entry: PluginEntry,
}

impl LoadedPlugin {
    pub fn load(entry: PluginEntry, library: LibraryId) -> Result<Self> {
        let env = environment(0);
        env.acquire(library)?;
        // SAFETY: DOC-12 §3，PluginEntry保证入口与描述符协议，HOST在进程期间有效。
        let result = unsafe { env.call(|out, err| (entry.function())(&HOST, library, out, err)) };
        match result {
            Ok(descriptor) => Ok(Self {
                descriptor,
                library,
                _entry: entry,
            }),
            Err(error) => {
                env.release(library)?;
                Err(error)
            }
        }
    }

    fn component(&self, index: usize) -> Result<&ComponentV1> {
        // SAFETY: DOC-12 §3、§10，本对象持有描述符及入口所引用代码的完整寿命。
        let descriptor = unsafe { &*self.descriptor };
        // SAFETY: DOC-12 §10，描述符库租约覆盖整张有效记录表，helper按各记录size定位组件。
        unsafe { component_at(descriptor, index) }
    }

    pub fn name(&self) -> Result<&str> {
        // SAFETY: DOC-12 §3、§10，描述符及名称在LoadedPlugin释放前有效。
        unsafe { (*self.descriptor).name.as_str() }
    }

    pub fn resolve(&self, index: usize, config: &[u8]) -> Result<()> {
        let component = self.component(index)?;
        // SAFETY: DOC-12 §10，resolve签名与配置借用有效，插件租约覆盖整个调用。
        unsafe { environment(0).call_unit(|err| (component.resolve)(AbiBytes::new(config), err)) }
    }

    pub fn apply(&self, index: usize, ctx: u64, config: &[u8]) -> Result<RemoteFuture<AbiUnit>> {
        let component = self.component(index)?;
        // SAFETY: DOC-12 §10，apply输出单位Future，配置借用覆盖构造调用。
        let raw = unsafe {
            environment(0).call(|out, err| (component.apply)(ctx, AbiBytes::new(config), out, err))
        }?;
        // SAFETY: DOC-12 §8、§10，apply按约定返回单位Future并转交租约。
        Ok(unsafe { RemoteFuture::from_raw(raw, environment(0)) })
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        let env = environment(0);
        let result =
            // SAFETY: DOC-12 §3、§10，LoadedPlugin独占描述符且在归还租约前调用源侧release。
            unsafe { env.call_unit(|err| ((*self.descriptor).release)(self.descriptor, err)) };
        let released = env.release(self.library);
        if let Err(error) = result.and(released) {
            env.report(error);
        }
    }
}
