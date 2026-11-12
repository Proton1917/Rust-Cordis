use cordis_abi::*;
use std::{
    collections::HashMap,
    mem::size_of,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy, Default)]
pub struct Counts {
    pub active: u64,
    pub acquired: u64,
    pub released: u64,
}

static NEXT_LIBRARY: AtomicU64 = AtomicU64::new(1);
static LIBRARIES: OnceLock<Mutex<HashMap<LibraryId, Counts>>> = OnceLock::new();
static MESSAGES: OnceLock<Mutex<Vec<(u32, String)>>> = OnceLock::new();
static NEXT_RESOURCE: AtomicU64 = AtomicU64::new(1);
static RESOURCES: OnceLock<Mutex<HashMap<u64, Arc<Object>>>> = OnceLock::new();

fn resources() -> &'static Mutex<HashMap<u64, Arc<Object>>> {
    RESOURCES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn libraries() -> &'static Mutex<HashMap<LibraryId, Counts>> {
    LIBRARIES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn environment() -> Environment {
    let library = NEXT_LIBRARY.fetch_add(1, Ordering::Relaxed);
    libraries()
        .lock()
        .unwrap()
        .insert(library, Counts::default());
    // SAFETY: DOC-12 §9，HOST实现租约、日志和退役上下文的错误及拥有输入处理。
    unsafe { Environment::new(&HOST, library) }
}

pub fn host_environment() -> Environment {
    // SAFETY: DOC-12 §9，HOST的函数均通过边界辅助初始化结果或错误。
    unsafe { Environment::new(&HOST, 0) }
}

pub fn counts(env: Environment) -> Counts {
    libraries().lock().unwrap()[&env.library()]
}

pub fn messages(env: Environment) -> Vec<(u32, String)> {
    let prefix = format!("library {}:", env.library());
    MESSAGES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, message)| message.starts_with(&prefix))
        .cloned()
        .collect()
}

fn retired<T>() -> Result<T> {
    Err(Error::new(4, "context has retired"))
}

pub static HOST: HostV1 = HostV1 {
    size: size_of::<HostV1>() as u64,
    log,
    now,
    sleep,
    spawn,
    effect,
    effect_async: effect,
    defer,
    defer_async: defer,
    is_current,
    provide: owned_object,
    get,
    on: owned_object,
    emit,
    emit_async,
    export_state,
    inherited_state,
    config,
    spawn_thread: owned_object,
    lease_acquire,
    lease_release,
    effect_settled: effect_future,
    effect_dispose: effect_future,
    object_export,
    object_import,
    object_release,
    effect_release,
};

// SAFETY: DOC-12 §9，message是调用期借用，err为可写槽。
unsafe extern "C" fn log(ctx: u64, level: u32, message: AbiStr, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§6，日志字符串在调用期间读取并复制。
    unsafe {
        boundary_unit(host_environment(), err, || {
            if ctx != 0 {
                return retired();
            }
            MESSAGES
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .unwrap()
                .push((level, message.as_str()?.to_owned()));
            Ok(())
        })
    }
}

// SAFETY: DOC-12 §9，out与err是该函数约定的结果槽。
unsafe extern "C" fn now(_: u64, out: *mut u64, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6，退役上下文写出code4，保持out原状。
    unsafe { boundary(host_environment(), out, err, retired) }
}
// SAFETY: DOC-12 §9，out与err是该函数约定的结果槽。
unsafe extern "C" fn sleep(_: u64, _: u64, out: *mut AbiFuture, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6，退役上下文返回拥有错误。
    unsafe { boundary(host_environment(), out, err, retired) }
}
// SAFETY: DOC-12 §5、§9，调用转交task与其既有租约。
unsafe extern "C" fn spawn(_: u64, task: AbiFuture, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§8，先接管单位Future再返回退役错误。
    unsafe {
        boundary_unit(host_environment(), err, || {
            let _task = RemoteFuture::<AbiUnit>::from_raw(task, host_environment());
            retired()
        })
    }
}
// SAFETY: DOC-12 §5、§9，调用转交body，out与err可写。
unsafe extern "C" fn effect(
    _: u64,
    _: AbiStr,
    body: AbiObject,
    out: *mut u64,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §5，失败路径仍接管并释放body。
    unsafe {
        boundary(host_environment(), out, err, || {
            let _body = Object::from_raw(body, host_environment());
            retired()
        })
    }
}
// SAFETY: DOC-12 §5、§9，调用转交cleanup及其租约。
unsafe extern "C" fn defer(_: u64, cleanup: AbiObject, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5，失败路径仍接管并释放cleanup。
    unsafe {
        boundary_unit(host_environment(), err, || {
            let _cleanup = Object::from_raw(cleanup, host_environment());
            retired()
        })
    }
}
// SAFETY: DOC-12 §9，out与err是该函数约定的结果槽。
unsafe extern "C" fn is_current(_: u64, out: *mut u8, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6，退役上下文返回错误。
    unsafe { boundary(host_environment(), out, err, retired) }
}
// SAFETY: DOC-12 §5、§9，调用转交value，name是调用期借用。
unsafe extern "C" fn owned_object(
    _: u64,
    _: AbiStr,
    value: AbiObject,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §5，宿主在报告错误前接管拥有输入。
    unsafe {
        boundary_unit(host_environment(), err, || {
            let _value = Object::from_raw(value, host_environment());
            retired()
        })
    }
}
// SAFETY: DOC-12 §9，out与err是该函数约定的结果槽。
unsafe extern "C" fn get(_: u64, _: AbiStr, out: *mut AbiObject, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6，结果槽在错误返回时保持原状。
    unsafe { boundary(host_environment(), out, err, retired) }
}
// SAFETY: DOC-12 §9，事件与载荷借用覆盖调用期。
unsafe extern "C" fn emit(_: u64, _: AbiStr, _: AbiBytes, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6，错误槽有效。
    unsafe { boundary_unit(host_environment(), err, retired) }
}
// SAFETY: DOC-12 §9，out与err是该函数约定的结果槽。
unsafe extern "C" fn emit_async(
    _: u64,
    _: AbiStr,
    _: AbiBytes,
    out: *mut AbiFuture,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §6，退役上下文写出拥有错误。
    unsafe { boundary(host_environment(), out, err, retired) }
}
// SAFETY: DOC-12 §5、§9，调用转交exporter及其租约。
unsafe extern "C" fn export_state(
    _: u64,
    _: AbiStr,
    _: u32,
    exporter: AbiObject,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §5，失败路径仍接管并释放导出器。
    unsafe {
        boundary_unit(host_environment(), err, || {
            let _exporter = Object::from_raw(exporter, host_environment());
            retired()
        })
    }
}
// SAFETY: DOC-12 §9，out与err是该函数约定的结果槽。
unsafe extern "C" fn inherited_state(
    _: u64,
    out: *mut AbiOption<AbiState>,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §6，退役上下文写出错误并保持out原状。
    unsafe { boundary(host_environment(), out, err, retired) }
}
// SAFETY: DOC-12 §9，out与err是该函数约定的结果槽。
unsafe extern "C" fn config(_: u64, out: *mut AbiBuf, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6，退役上下文写出错误。
    unsafe { boundary(host_environment(), out, err, retired) }
}
// SAFETY: DOC-12 §9，out与err是该函数约定的结果槽。
unsafe extern "C" fn effect_future(_: u64, out: *mut AbiFuture, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6，退役句柄写出错误。
    unsafe { boundary(host_environment(), out, err, retired) }
}
// SAFETY: DOC-12 §9，effect使用宿主编号，err为有效错误槽。
unsafe extern "C" fn effect_release(_: u64, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6、§9，该宿主保持退役上下文状态，返回对应句柄错误。
    unsafe { boundary_unit(host_environment(), err, retired) }
}
// SAFETY: DOC-12 §11，library是已登记的库编号，err是可写槽。
unsafe extern "C" fn lease_acquire(library: LibraryId, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6、§11，计数修改和错误构造位于边界内。
    unsafe {
        boundary_unit(host_environment(), err, || {
            let mut libraries = libraries().lock().unwrap();
            let entry = libraries
                .get_mut(&library)
                .ok_or_else(|| Error::new(2, "unknown library"))?;
            entry.active += 1;
            entry.acquired += 1;
            Ok(())
        })
    }
}
// SAFETY: DOC-12 §11，调用者归还一份既有租约，err是可写槽。
unsafe extern "C" fn lease_release(library: LibraryId, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §6、§11，计数归还和错误构造位于边界内。
    unsafe {
        boundary_unit(host_environment(), err, || {
            let mut libraries = libraries().lock().unwrap();
            let entry = libraries
                .get_mut(&library)
                .ok_or_else(|| Error::new(2, "unknown library"))?;
            entry.active = entry
                .active
                .checked_sub(1)
                .ok_or_else(|| Error::new(2, "lease already released"))?;
            entry.released += 1;
            Ok(())
        })
    }
}

// SAFETY: DOC-12 §5、§9，调用转交一个拥有对象，out与err为有效输出槽。
unsafe extern "C" fn object_export(raw: AbiObject, out: *mut u64, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§9，资源表取得源侧对象和租约，编号由宿主分配。
    unsafe {
        boundary(host_environment(), out, err, || {
            let object = Object::from_raw(raw, host_environment());
            let id = NEXT_RESOURCE.fetch_add(1, Ordering::Relaxed);
            resources().lock().unwrap().insert(id, Arc::new(object));
            Ok(id)
        })
    }
}

// SAFETY: DOC-12 §9，out与err为有效输出槽，资源编号由宿主查表。
unsafe extern "C" fn object_import(id: u64, out: *mut AbiObject, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §7、§9，锁外clone产生独立拥有对象。
    unsafe {
        boundary(host_environment(), out, err, || {
            let object = resources()
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .ok_or_else(|| Error::new(4, "resource has been released"))?;
            object.try_clone().map(Object::into_raw)
        })
    }
}

// SAFETY: DOC-12 §9，err为有效错误槽，资源表记录在锁外释放。
unsafe extern "C" fn object_release(id: u64, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§9，移出对象后交回资源表持有的一份引用。
    unsafe {
        boundary_unit(host_environment(), err, || {
            let object = resources()
                .lock()
                .unwrap()
                .remove(&id)
                .ok_or_else(|| Error::new(4, "resource has been released"))?;
            drop(object);
            Ok(())
        })
    }
}
