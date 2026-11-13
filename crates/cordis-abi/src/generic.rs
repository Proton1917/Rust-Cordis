use crate::{
    AbiBuf, AbiBytes, AbiError, AbiFuture, AbiStatus, AbiStr, Buf, Environment, Error, Object,
    ObjectHeader, RemoteFuture, Result, boundary, make_future, object_data, object_environment,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    ffi::c_void,
    future::Future,
    io::Cursor,
    mem::size_of,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

pub use ciborium::value::Value as CborValue;
pub const RESOURCE_TAG: u64 = 60000;
pub const OPTION_SOME_TAG: u64 = 60001;

pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<CborValue> {
    crate::encoding::plain(|| {
        CborValue::serialized(value).map_err(|error| Error::new(2, error.to_string()))
    })
}

pub fn from_value<T: DeserializeOwned>(value: CborValue) -> Result<T> {
    crate::encoding::plain(|| {
        value
            .deserialized()
            .map_err(|error| Error::new(2, error.to_string()))
    })
}

pub fn decode_value(bytes: &[u8]) -> Result<CborValue> {
    let mut input = Cursor::new(bytes);
    let value =
        ciborium::from_reader(&mut input).map_err(|error| Error::new(2, error.to_string()))?;
    if input.position() != bytes.len() as u64 {
        return Err(Error::new(2, "CBOR payload contains trailing bytes"));
    }
    Ok(value)
}

pub fn decode_arguments(bytes: &[u8]) -> Result<Vec<CborValue>> {
    match decode_value(bytes)? {
        CborValue::Array(values) => Ok(values),
        _ => Err(Error::new(
            2,
            "generic invocation arguments must be a CBOR array",
        )),
    }
}

pub fn option_some(value: CborValue) -> CborValue {
    match value {
        CborValue::Null | CborValue::Tag(OPTION_SOME_TAG, _) => {
            CborValue::Tag(OPTION_SOME_TAG, Box::new(value))
        }
        value => value,
    }
}

pub fn option_take(value: CborValue) -> Option<CborValue> {
    match value {
        CborValue::Null => None,
        CborValue::Tag(OPTION_SOME_TAG, value) => Some(*value),
        value => Some(value),
    }
}

pub struct InvokeFuture {
    future: RemoteFuture<AbiBuf>,
    env: Environment,
}

impl InvokeFuture {
    // SAFETY: DOC-12 §7、§8，raw是输出拥有AbiBuf的有效Future。
    pub(crate) unsafe fn from_raw(raw: AbiFuture, env: Environment) -> Self {
        // SAFETY: DOC-12 §8，输出类型与通用调用约定一致，既有租约随raw转入。
        let future = unsafe { RemoteFuture::from_raw(raw, env) };
        Self { future, env }
    }
    pub fn into_raw(self) -> AbiFuture {
        self.future.into_raw()
    }
}

impl Future for InvokeFuture {
    type Output = Result<Buf>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.future).poll(cx) {
            Poll::Ready(Ok(raw)) => {
                // SAFETY: DOC-12 §7、§8，Ready交出CBOR缓冲与附带资源守卫。
                Poll::Ready(Ok(unsafe { Buf::from_raw(raw, self.env) }))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub fn make_invoke_future<F>(future: F, env: Environment) -> Result<InvokeFuture>
where
    F: Future<Output = Result<Buf>> + Send + 'static,
{
    let future = make_future(async move { future.await.map(Buf::into_raw) }, env)?;
    // SAFETY: DOC-12 §7、§8，包装函数产生拥有AbiBuf的Future。
    Ok(unsafe { InvokeFuture::from_raw(future.into_raw(), env) })
}

/// DOC-12 §7：异步入口在返回Future前取得全部参数资源的独立引用。
pub trait GenericService: Send + Sync + 'static {
    fn invoke(&self, method: &str, args: &[u8]) -> Result<Buf>;
    fn invoke_async(&self, method: &str, args: &[u8]) -> Result<InvokeFuture>;
}

struct Dispatch {
    header: ObjectHeader,
    _interface: String,
    _method_names: Vec<String>,
    _method_table: Vec<AbiStr>,
    service: Arc<dyn GenericService>,
}

pub fn generic_object(
    interface: String,
    methods: Vec<String>,
    service: Arc<dyn GenericService>,
    env: Environment,
) -> Result<Object> {
    let table: Vec<_> = methods.iter().map(|name| AbiStr::new(name)).collect();
    let mut header =
        ObjectHeader::for_type::<Dispatch>(size_of::<ObjectHeader>(), 1, "").with_flags(0);
    header.interface = AbiStr::new(&interface);
    header.methods = table.as_ptr();
    header.method_count = table.len() as u64;
    header.invoke = dispatch;
    header.invoke_async = dispatch_async;
    let value = Arc::new(Dispatch {
        header,
        _interface: interface,
        _method_names: methods,
        _method_table: table,
        service,
    });
    let header = std::ptr::from_ref(&value.header);
    // SAFETY: DOC-12 §7，整个通用虚表位于最终Arc内，克隆对象共享其只读存储和方法名。
    unsafe { Object::new(value, header, env) }
}

// SAFETY: DOC-12 §7，data对应Dispatch，借用参数及输出槽覆盖调用期。
unsafe extern "C" fn dispatch(
    data: *mut c_void,
    method: AbiStr,
    args: AbiBytes,
    out: *mut AbiBuf,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §7，data具有有效Object公共前缀。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §5、§7，用户调用位于边界内，拥有结果随out转交。
    unsafe {
        boundary(env, out, err, || {
            object_data::<Dispatch>(data)
                .service
                .invoke(method.as_str()?, args.as_slice())
                .map(Buf::into_raw)
        })
    }
}

// SAFETY: DOC-12 §7、§8，data对应Dispatch，Future结果类型为AbiBuf。
unsafe extern "C" fn dispatch_async(
    data: *mut c_void,
    method: AbiStr,
    args: AbiBytes,
    out: *mut AbiFuture,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §7，data具有有效Object公共前缀。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §7、§8，服务入口取得参数资源后交出独立Future。
    unsafe {
        boundary(env, out, err, || {
            object_data::<Dispatch>(data)
                .service
                .invoke_async(method.as_str()?, args.as_slice())
                .map(InvokeFuture::into_raw)
        })
    }
}
