use crate::{
    AbiBuf, AbiBytes, AbiError, AbiObject, AbiStatus, AbiStr, AbiUnit, AsyncCall, Buf, CborValue,
    Environment, Error, Invoke, InvokeAsync, InvokeFuture, OBJECT_TYPED_METHODS, ObjectHeader,
    RemoteFuture, Result, SyncCall, boundary, error::release_boundary,
};
use std::{ffi::c_void, mem::size_of, sync::Arc};

#[repr(C)]
struct ObjectPrefix {
    env: Environment,
    vtable: *const ObjectHeader,
    scope: Option<ScopeFactory>,
}
#[repr(C)]
struct ObjectData<T: ?Sized> {
    prefix: ObjectPrefix,
    value: Arc<T>,
}

pub struct Object {
    raw: Option<AbiObject>,
    env: Environment,
}

pub type ScopeFactory = Arc<dyn Fn(CborValue) -> Result<Object> + Send + Sync>;

// SAFETY: DOC-12 §2、§7，接口实现及回调支持跨线程，Object持有库租约。
unsafe impl Send for Object {}
// SAFETY: DOC-12 §2、§7，对象的共享调用遵循Send+Sync契约。
unsafe impl Sync for Object {}

impl ObjectHeader {
    pub const fn for_type<T: ?Sized + Send + Sync + 'static>(
        size: usize,
        kind: u32,
        interface: &'static str,
    ) -> Self {
        Self {
            size: size as u64,
            kind,
            flags: if kind == 1 { OBJECT_TYPED_METHODS } else { 0 },
            reserved: 0,
            interface: AbiStr::new(interface),
            clone: clone_object::<T>,
            drop: drop_object::<T>,
            methods: std::ptr::null(),
            method_count: 0,
            invoke: missing_invoke,
            invoke_async: missing_invoke_async,
        }
    }

    pub const fn with_dispatch(
        mut self,
        methods: &'static [AbiStr],
        invoke: Invoke,
        invoke_async: InvokeAsync,
    ) -> Self {
        self.methods = methods.as_ptr();
        self.method_count = methods.len() as u64;
        self.invoke = invoke;
        self.invoke_async = invoke_async;
        self
    }

    pub const fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }
}

impl Object {
    /// # Safety
    /// DOC-12 §7：vtable从完整只读虚表派生，覆盖size声明的字节；首部由
    /// for_type<T>构造，方法与T匹配，方法名表及其UTF-8文本由静态存储或T保持存活。
    // SAFETY: DOC-12 §7 的类型、完整虚表来源和库寿命由构造者保证。
    pub unsafe fn new<T: ?Sized + Send + Sync + 'static>(
        value: Arc<T>,
        vtable: *const ObjectHeader,
        env: Environment,
    ) -> Result<Self> {
        // SAFETY: DOC-12 §7，类型与完整虚表的关联继承调用者契约。
        unsafe { Self::create(value, vtable, env, None) }
    }

    /// # Safety
    /// DOC-12 §7：vtable满足new的类型与完整范围契约；scope在原对象库内生成视图。
    // SAFETY: DOC-12 §7，作用域工厂及其捕获由对象和库租约共同持有。
    pub unsafe fn new_scoped<T: ?Sized + Send + Sync + 'static>(
        value: Arc<T>,
        vtable: *const ObjectHeader,
        env: Environment,
        scope: ScopeFactory,
    ) -> Result<Self> {
        // SAFETY: DOC-12 §7，工厂与原对象共享源侧生命周期。
        unsafe { Self::create(value, vtable, env, Some(scope)) }
    }

    // SAFETY: DOC-12 §7，调用入口确认T、虚表及可选工厂来自同一源侧实现。
    unsafe fn create<T: ?Sized + Send + Sync + 'static>(
        value: Arc<T>,
        vtable: *const ObjectHeader,
        env: Environment,
        scope: Option<ScopeFactory>,
    ) -> Result<Self> {
        env.acquire(env.library())?;
        let data = Box::new(ObjectData {
            prefix: ObjectPrefix { env, vtable, scope },
            value,
        });
        Ok(Self {
            raw: Some(AbiObject {
                data: Box::into_raw(data).cast(),
                vtable,
                library: env.library(),
            }),
            env,
        })
    }

    /// # Safety
    /// DOC-12 §7、§11：raw的vtable指针覆盖完整虚表，唯一拥有引用和既有租约转入。
    // SAFETY: DOC-12 §5、§11 的拥有值由边界调用者转交。
    pub unsafe fn from_raw(raw: AbiObject, env: Environment) -> Self {
        Self {
            raw: Some(raw),
            env,
        }
    }

    pub fn into_raw(mut self) -> AbiObject {
        self.raw.take().expect("object is owned")
    }
    pub fn data(&self) -> *mut c_void {
        self.raw.as_ref().expect("object is owned").data
    }
    pub fn environment(&self) -> Environment {
        self.env
    }
    pub fn library(&self) -> u64 {
        self.raw.as_ref().expect("object is owned").library
    }
    pub fn header(&self) -> &ObjectHeader {
        // SAFETY: DOC-12 §7、§11，self的租约保持整个vtable的寿命。
        unsafe { &*self.raw.as_ref().expect("object is owned").vtable }
    }
    pub fn interface(&self) -> Result<&str> {
        // SAFETY: DOC-12 §7，借用受self及其库租约约束。
        unsafe { self.header().interface.as_str() }
    }
    pub fn has_typed_methods(&self) -> bool {
        self.header().flags & OBJECT_TYPED_METHODS != 0
    }
    pub fn method_names(&self) -> Result<Vec<String>> {
        let header = self.header();
        if header.method_count == 0 {
            return Ok(Vec::new());
        }
        // SAFETY: DOC-12 §7，拥有对象保持整个方法名表及各字符串的寿命。
        unsafe { std::slice::from_raw_parts(header.methods, header.method_count as usize) }
            .iter()
            .map(|name| {
                // SAFETY: DOC-12 §7，字符串借用期受拥有对象约束。
                unsafe { name.as_str() }.map(str::to_owned)
            })
            .collect()
    }
    pub fn invoke(&self, method: &str, args: &[u8]) -> Result<Buf> {
        // SAFETY: DOC-12 §7，通用入口交出拥有CBOR缓冲，借用参数覆盖本次调用。
        let raw = unsafe {
            self.env.call(|out, err| {
                (self.header().invoke)(
                    self.data(),
                    AbiStr::new(method),
                    AbiBytes::new(args),
                    out,
                    err,
                )
            })
        }?;
        // SAFETY: DOC-12 §5、§7，成功调用转交结果缓冲及资源守卫。
        Ok(unsafe { Buf::from_raw(raw, self.env) })
    }
    pub fn invoke_async(&self, method: &str, args: &[u8]) -> Result<InvokeFuture> {
        // SAFETY: DOC-12 §7、§8，异步通用入口输出AbiBuf结果的Future。
        let raw = unsafe {
            self.env.call(|out, err| {
                (self.header().invoke_async)(
                    self.data(),
                    AbiStr::new(method),
                    AbiBytes::new(args),
                    out,
                    err,
                )
            })
        }?;
        // SAFETY: DOC-12 §7、§8，入口已取得参数资源，Future持有独立的拥有引用。
        Ok(unsafe { InvokeFuture::from_raw(raw, self.env) })
    }
    pub fn try_clone(&self) -> Result<Self> {
        // SAFETY: DOC-12 §5、§7，clone输出转交一份新拥有引用与租约。
        let raw = unsafe {
            self.env
                .call(|out, err| (self.header().clone)(self.data(), out, err))
        }?;
        // SAFETY: DOC-12 §7、§11，clone已生成有效的拥有对象。
        Ok(unsafe { Self::from_raw(raw, self.env) })
    }

    /// # Safety
    /// DOC-12 §7.2：F为该槽声明的C函数指针类型，调用时遵守方法参数契约。
    // SAFETY: DOC-12 §7.2 的函数指针类型由接口宏的声明顺序保证。
    pub unsafe fn typed_slot<F: Copy>(&self, index: usize) -> Result<F> {
        if self.header().kind == 1 && !self.has_typed_methods() {
            return Err(Error::new(3, "object exposes generic invocation"));
        }
        let end = index
            .checked_add(1)
            .and_then(|n| n.checked_mul(8))
            .and_then(|n| n.checked_add(size_of::<ObjectHeader>()))
            .ok_or_else(|| Error::new(3, "method slot exceeds vtable size"))?;
        if self.header().size < end as u64 {
            return Err(Error::new(3, "method is absent from provider vtable"));
        }
        let vtable = self.raw.as_ref().expect("object is owned").vtable;
        // SAFETY: DOC-12 §7.2，size覆盖该槽，F是对应的C函数指针。
        Ok(unsafe { vtable.cast::<u8>().add(end - 8).cast::<F>().read() })
    }

    pub fn call_sync(&self, ctx: u64, scope: u64, payload: &[u8]) -> Result<()> {
        self.require_kind(2)?;
        // SAFETY: DOC-12 §7.4，kind2的首槽是SyncCall。
        let call = unsafe { self.typed_slot::<SyncCall>(0) }?;
        // SAFETY: DOC-12 §7.4，kind2的首槽是SyncCall，借用payload覆盖调用期。
        unsafe {
            self.env
                .call_unit(|err| call(self.data(), ctx, scope, AbiBytes::new(payload), err))
        }
    }
    pub fn call_async(
        &self,
        ctx: u64,
        scope: u64,
        payload: &[u8],
    ) -> Result<RemoteFuture<AbiUnit>> {
        self.require_kind(3)?;
        // SAFETY: DOC-12 §7.4，kind3的首槽是AsyncCall。
        let call = unsafe { self.typed_slot::<AsyncCall>(0) }?;
        // SAFETY: DOC-12 §7.4、§8，异步回调输出单位Future。
        let raw = unsafe {
            self.env
                .call(|out, err| call(self.data(), ctx, scope, AbiBytes::new(payload), out, err))
        }?;
        // SAFETY: DOC-12 §8，已取得单位Future及其租约。
        Ok(unsafe { RemoteFuture::from_raw(raw, self.env) })
    }
    pub fn export(&self) -> Result<Buf> {
        self.require_kind(4)?;
        type Export = unsafe extern "C" fn(*mut c_void, *mut AbiBuf, *mut AbiError) -> AbiStatus;
        // SAFETY: DOC-12 §7.4，kind4首槽是export。
        let call = unsafe { self.typed_slot::<Export>(0) }?;
        // SAFETY: DOC-12 §5、§7.4，输出为拥有缓冲。
        let raw = unsafe { self.env.call(|out, err| call(self.data(), out, err)) }?;
        // SAFETY: DOC-12 §5，导出调用转交缓冲与租约。
        Ok(unsafe { Buf::from_raw(raw, self.env) })
    }
    pub fn thaw(&self) -> Result<()> {
        self.require_kind(4)?;
        type Thaw = unsafe extern "C" fn(*mut c_void, *mut AbiError) -> AbiStatus;
        // SAFETY: DOC-12 §7.4，kind4第二槽是thaw。
        let call = unsafe { self.typed_slot::<Thaw>(1) }?;
        // SAFETY: DOC-12 §7.4，对象保持导出器与调用代码存活。
        unsafe { self.env.call_unit(|err| call(self.data(), err)) }
    }
    fn require_kind(&self, kind: u32) -> Result<()> {
        if self.header().kind == kind {
            Ok(())
        } else {
            Err(Error::new(2, "callback kind mismatch"))
        }
    }
    pub fn release(mut self) -> Result<()> {
        self.release_inner()
    }
    fn release_inner(&mut self) -> Result<()> {
        let Some(raw) = self.raw.take() else {
            return Ok(());
        };
        // SAFETY: DOC-12 §7、§11，对象及其vtable在释放回调返回前存活。
        let result = unsafe {
            self.env
                .call_unit(|err| ((*raw.vtable).drop)(raw.data, err))
        };
        let lease = self.env.release(raw.library);
        result.and(lease)
    }
}
impl Drop for Object {
    fn drop(&mut self) {
        if let Err(error) = self.release_inner() {
            self.env.report(error);
        }
    }
}

/// # Safety
/// DOC-12 §7：data由Object::new<T>产生，拥有对象覆盖借用期。
// SAFETY: DOC-12 §7 的具体对象类型由宏生成的蹦床保证。
pub unsafe fn object_data<'a, T: ?Sized>(data: *mut c_void) -> &'a T {
    // SAFETY: DOC-12 §7，data对应ObjectData<T>且其Arc保持T存活。
    unsafe { &(*data.cast::<ObjectData<T>>()).value }
}

/// # Safety
/// DOC-12 §7：data由Object::new<T>产生，调用期间对象有效。
// SAFETY: DOC-12 §7 的具体对象类型由宏生成的蹦床保证。
pub unsafe fn object_arc<T: ?Sized>(data: *mut c_void) -> Arc<T> {
    // SAFETY: DOC-12 §7，复制当前有效ObjectData<T>中的Arc。
    unsafe { (*data.cast::<ObjectData<T>>()).value.clone() }
}

/// # Safety
/// DOC-12 §7：data由本crate的Object::new产生，调用期有效。
// SAFETY: DOC-12 §7 的公共前缀由ObjectData的repr(C)布局保证。
pub unsafe fn object_environment(data: *mut c_void) -> Environment {
    // SAFETY: DOC-12 §7，ObjectPrefix位于每个ObjectData的首部。
    unsafe { (*data.cast::<ObjectPrefix>()).env }
}

/// # Safety
/// DOC-12 §7：data由本crate创建，调用期由拥有对象保持有效。
// SAFETY: DOC-12 §7，工厂从对象公共前缀取得，用户代码由调用方的ABI边界捕获panic。
pub unsafe fn object_scope(data: *mut c_void, metadata: CborValue) -> Result<Object> {
    // SAFETY: DOC-12 §7，公共前缀与对象保持同一生命周期。
    let scope = unsafe { &*data.cast::<ObjectPrefix>() }
        .scope
        .clone()
        .ok_or_else(|| Error::new(3, "对象提供普通接口"))?;
    scope(metadata)
}

// SAFETY: DOC-12 §7，vtable把本蹦床与ObjectData<T>配套。
unsafe extern "C" fn clone_object<T: ?Sized + Send + Sync + 'static>(
    data: *mut c_void,
    out: *mut AbiObject,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §7，原拥有引用覆盖调用期间。
    let source = unsafe { &*data.cast::<ObjectData<T>>() };
    // SAFETY: DOC-12 §5、§7，输出槽有效，保留匹配T的完整虚表原始指针。
    unsafe {
        boundary(source.prefix.env, out, err, || {
            Object::create(
                source.value.clone(),
                source.prefix.vtable,
                source.prefix.env,
                source.prefix.scope.clone(),
            )
            .map(Object::into_raw)
        })
    }
}

// SAFETY: DOC-12 §7，调用者转交ObjectData<T>的唯一Box拥有权。
unsafe extern "C" fn drop_object<T: ?Sized>(data: *mut c_void, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§6，源侧Box与其用户析构一起在边界内释放。
    unsafe { release_boundary(err, || drop(Box::from_raw(data.cast::<ObjectData<T>>()))) }
}

// SAFETY: DOC-12 §7，data具有Object公共前缀，结果与错误槽有效。
unsafe extern "C" fn missing_invoke(
    data: *mut c_void,
    _: AbiStr,
    _: AbiBytes,
    out: *mut AbiBuf,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §6、§7，缺席方法通过code3返回，输出保持原状。
    unsafe {
        boundary(object_environment(data), out, err, || {
            Err(Error::new(3, "unknown method"))
        })
    }
}
// SAFETY: DOC-12 §7、§8，data具有Object公共前缀，Future输出槽有效。
unsafe extern "C" fn missing_invoke_async(
    data: *mut c_void,
    _: AbiStr,
    _: AbiBytes,
    out: *mut crate::AbiFuture,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §6、§7，缺席异步方法通过code3返回。
    unsafe {
        boundary(object_environment(data), out, err, || {
            Err(Error::new(3, "unknown method"))
        })
    }
}
