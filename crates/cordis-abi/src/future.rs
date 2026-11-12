use crate::{
    AbiError, AbiFuture, AbiStatus, AbiWaker, Environment, Error, FutureVTable, Result,
    WakerVTable, boundary, boundary_unit, error::release_boundary,
};
use std::{
    ffi::c_void,
    future::Future,
    marker::PhantomData,
    mem::{MaybeUninit, size_of},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

pub struct RemoteFuture<T> {
    raw: Option<AbiFuture>,
    env: Environment,
    finished: bool,
    output: PhantomData<fn() -> T>,
}

// SAFETY: DOC-12 §8，唯一拥有者串行poll/drop，Future实现满足Send契约。
unsafe impl<T> Send for RemoteFuture<T> {}

impl<T> RemoteFuture<T> {
    /// # Safety
    /// DOC-12 §8、§11：raw有效、输出布局为T，唯一引用与租约转入。
    // SAFETY: DOC-12 §8 的输出类型和所有权由调用方法确定。
    pub unsafe fn from_raw(raw: AbiFuture, env: Environment) -> Self {
        Self {
            raw: Some(raw),
            env,
            finished: false,
            output: PhantomData,
        }
    }
    pub fn into_raw(mut self) -> AbiFuture {
        self.raw.take().expect("future is owned")
    }
    pub fn release(mut self) -> Result<()> {
        self.release_inner()
    }
    fn release_inner(&mut self) -> Result<()> {
        let Some(raw) = self.raw.take() else {
            return Ok(());
        };
        // SAFETY: DOC-12 §8，独占Future且当前poll已经结束。
        let result = unsafe {
            self.env
                .call_unit(|err| ((*raw.vtable).drop)(raw.data, err))
        };
        let lease = self.env.release(raw.library);
        result.and(lease)
    }
}

impl<T> Future for RemoteFuture<T> {
    type Output = Result<T>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        assert!(!self.finished, "future polled after completion");
        let waker = match LocalWaker::new(cx.waker().clone(), self.env) {
            Ok(waker) => waker,
            Err(error) => {
                self.finished = true;
                return Poll::Ready(Err(error));
            }
        };
        let raw = self.raw.as_ref().expect("future is owned");
        let mut output = MaybeUninit::<T>::uninit();
        let mut state = 0;
        // SAFETY: DOC-12 §8，T匹配该Future输出，waker和输出槽覆盖整个poll。
        let result = unsafe {
            self.env.call_unit(|err| {
                ((*raw.vtable).poll)(
                    raw.data,
                    waker.raw.as_ref().expect("waker is owned"),
                    output.as_mut_ptr().cast(),
                    &mut state,
                    err,
                )
            })
        };
        match result {
            Err(error) => {
                self.finished = true;
                Poll::Ready(Err(error))
            }
            Ok(()) if state == 0 => Poll::Pending,
            Ok(()) if state == 1 => {
                self.finished = true;
                // SAFETY: DOC-12 §8，Ready成功初始化了T。
                Poll::Ready(Ok(unsafe { output.assume_init() }))
            }
            Ok(()) => {
                self.finished = true;
                Poll::Ready(Err(Error::new(2, "invalid Future poll state")))
            }
        }
    }
}
impl<T> Drop for RemoteFuture<T> {
    fn drop(&mut self) {
        if let Err(error) = self.release_inner() {
            self.env.report(error);
        }
    }
}

struct FutureData<F> {
    future: Pin<Box<F>>,
    env: Environment,
}

impl<F, T> FutureData<F>
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: 'static,
{
    const VTABLE: FutureVTable = FutureVTable {
        size: size_of::<FutureVTable>() as u64,
        poll: poll_future::<F, T>,
        drop: drop_future::<F>,
    };
}

pub fn make_future<F, T>(future: F, env: Environment) -> Result<RemoteFuture<T>>
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: 'static,
{
    env.acquire(env.library())?;
    let data = Box::new(FutureData {
        future: Box::pin(future),
        env,
    });
    let raw = AbiFuture {
        data: Box::into_raw(data).cast(),
        vtable: &FutureData::<F>::VTABLE,
        library: env.library(),
    };
    // SAFETY: DOC-12 §8，泛型蹦床与T匹配，租约已经取得。
    Ok(unsafe { RemoteFuture::from_raw(raw, env) })
}

// SAFETY: DOC-12 §8，调用者串行poll，out布局为T，waker是调用期借用。
unsafe extern "C" fn poll_future<F, T>(
    data: *mut c_void,
    waker: *const AbiWaker,
    out: *mut c_void,
    state: *mut u32,
    err: *mut AbiError,
) -> AbiStatus
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: 'static,
{
    // SAFETY: DOC-12 §8，data由make_future<F,T>创建并独占。
    let data = unsafe { &mut *data.cast::<FutureData<F>>() };
    let mut result = MaybeUninit::<Option<T>>::uninit();
    // SAFETY: DOC-12 §5、§8，局部结果槽有效，用户poll完整位于panic边界内。
    let status = unsafe {
        boundary(data.env, result.as_mut_ptr(), err, || {
            let waker = ForeignWake::from_borrowed(&*waker, data.env)?;
            match data.future.as_mut().poll(&mut Context::from_waker(&waker)) {
                Poll::Pending => Ok(None),
                Poll::Ready(value) => value.map(Some),
            }
        })
    };
    if status == 0 {
        // SAFETY: DOC-12 §5，成功边界初始化了局部Option。
        match unsafe { result.assume_init() } {
            Some(value) => {
                // SAFETY: DOC-12 §8，输出T与Ready状态一起交出。
                unsafe {
                    out.cast::<T>().write(value);
                    state.write(1);
                }
            }
            None => {
                // SAFETY: DOC-12 §8，Pending只写状态。
                unsafe { state.write(0) };
            }
        }
    }
    status
}

// SAFETY: DOC-12 §8，调用者转交FutureData<F>，poll已经结束。
unsafe extern "C" fn drop_future<F>(data: *mut c_void, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§6，Future及其保存的Waker在源侧析构。
    unsafe { release_boundary(err, || drop(Box::from_raw(data.cast::<FutureData<F>>()))) }
}

struct WakerData {
    waker: Waker,
    env: Environment,
}
struct LocalWaker {
    raw: Option<AbiWaker>,
    env: Environment,
}

impl LocalWaker {
    fn new(waker: Waker, env: Environment) -> Result<Self> {
        env.acquire(env.library())?;
        Ok(Self {
            raw: Some(AbiWaker {
                data: Box::into_raw(Box::new(WakerData { waker, env })).cast(),
                vtable: &WAKER_VTABLE,
                library: env.library(),
            }),
            env,
        })
    }
    fn into_raw(mut self) -> AbiWaker {
        self.raw.take().expect("waker is owned")
    }
}
impl Drop for LocalWaker {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: DOC-12 §8，LocalWaker拥有这一Waker引用。
            let result = unsafe {
                self.env
                    .call_unit(|err| ((*raw.vtable).drop)(raw.data, err))
            };
            let lease = self.env.release(raw.library);
            if let Err(error) = result.and(lease) {
                self.env.report(error);
            }
        }
    }
}

static WAKER_VTABLE: WakerVTable = WakerVTable {
    clone: clone_waker,
    wake: wake_waker,
    wake_by_ref: wake_waker_by_ref,
    drop: drop_waker,
};

// SAFETY: DOC-12 §8，data是有效WakerData，输出槽可写。
unsafe extern "C" fn clone_waker(
    data: *mut c_void,
    out: *mut AbiWaker,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §8，源引用在clone调用期间存活。
    let data = unsafe { &*data.cast::<WakerData>() };
    // SAFETY: DOC-12 §5、§8，新Waker取得独立租约后交出。
    unsafe {
        boundary(data.env, out, err, || {
            LocalWaker::new(data.waker.clone(), data.env).map(LocalWaker::into_raw)
        })
    }
}
// SAFETY: DOC-12 §8，调用转交WakerData的唯一引用。
unsafe extern "C" fn wake_waker(data: *mut c_void, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§8，消费Box后消费其Waker引用。
    unsafe {
        release_boundary(err, || {
            let WakerData { waker, .. } = *Box::from_raw(data.cast::<WakerData>());
            waker.wake();
        })
    }
}
// SAFETY: DOC-12 §8，调用期保持WakerData有效。
unsafe extern "C" fn wake_waker_by_ref(data: *mut c_void, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §8，只借用当前拥有的Waker。
    let data = unsafe { &*data.cast::<WakerData>() };
    // SAFETY: DOC-12 §6、§8，用户唤醒操作位于边界内。
    unsafe {
        boundary_unit(data.env, err, || {
            data.waker.wake_by_ref();
            Ok(())
        })
    }
}
// SAFETY: DOC-12 §8，调用转交WakerData的唯一Box引用。
unsafe extern "C" fn drop_waker(data: *mut c_void, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§6，源侧释放Waker并捕获析构panic。
    unsafe { release_boundary(err, || drop(Box::from_raw(data.cast::<WakerData>()))) }
}

struct ForeignWake {
    raw: Option<AbiWaker>,
    env: Environment,
}
// SAFETY: DOC-12 §8，外部Waker操作支持任意线程，拥有引用保持租约。
unsafe impl Send for ForeignWake {}
// SAFETY: DOC-12 §8，wake_by_ref与clone支持共享访问。
unsafe impl Sync for ForeignWake {}

impl ForeignWake {
    // SAFETY: DOC-12 §8，raw是poll调用期内有效的借用Waker。
    unsafe fn from_borrowed(raw: &AbiWaker, env: Environment) -> Result<Waker> {
        // SAFETY: DOC-12 §8，clone交出新的拥有引用及租约。
        let owned = unsafe { env.call(|out, err| ((*raw.vtable).clone)(raw.data, out, err)) }?;
        Ok(Waker::from(Arc::new(Self {
            raw: Some(owned),
            env,
        })))
    }
    fn consume(&mut self, wake: bool) {
        let Some(raw) = self.raw.take() else {
            return;
        };
        // SAFETY: DOC-12 §8，消费操作使用这一独立拥有引用。
        let result = unsafe {
            self.env.call_unit(|err| {
                if wake {
                    ((*raw.vtable).wake)(raw.data, err)
                } else {
                    ((*raw.vtable).drop)(raw.data, err)
                }
            })
        };
        let lease = self.env.release(raw.library);
        if let Err(error) = result.and(lease) {
            self.env.report(error);
        }
    }
}
impl Wake for ForeignWake {
    fn wake(self: Arc<Self>) {
        match Arc::try_unwrap(self) {
            Ok(mut owned) => owned.consume(true),
            Err(shared) => shared.wake_by_ref(),
        }
    }
    fn wake_by_ref(self: &Arc<Self>) {
        let raw = self.raw.as_ref().expect("waker is owned");
        // SAFETY: DOC-12 §8，Arc保持引用与租约，调用为共享唤醒。
        if let Err(error) = unsafe {
            self.env
                .call_unit(|err| ((*raw.vtable).wake_by_ref)(raw.data, err))
        } {
            self.env.report(error);
        }
    }
}
impl Drop for ForeignWake {
    fn drop(&mut self) {
        self.consume(false);
    }
}
