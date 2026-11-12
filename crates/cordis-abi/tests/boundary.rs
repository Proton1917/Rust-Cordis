#[path = "support/generic.rs"]
mod generic_tests;
mod support;

use cordis_abi::*;
use serde::{Deserialize, Serialize};
use std::{
    ffi::c_void,
    future::Future,
    mem::{ManuallyDrop, MaybeUninit, size_of},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

struct Service {
    prefix: Vec<u8>,
    env: Environment,
    released: Arc<AtomicUsize>,
}
impl Drop for Service {
    fn drop(&mut self) {
        assert!(
            support::counts(self.env).active > 0,
            "资源析构期间库租约有效"
        );
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

type Transform = unsafe extern "C" fn(*mut c_void, AbiBuf, *mut AbiBuf, *mut AbiError) -> AbiStatus;
type Sum = unsafe extern "C" fn(*mut c_void, i64, i64, *mut i64, *mut AbiError) -> AbiStatus;
type Join =
    unsafe extern "C" fn(*mut c_void, AbiBuf, AbiBuf, *mut AbiBuf, *mut AbiError) -> AbiStatus;
type Panics = unsafe extern "C" fn(*mut c_void, *mut AbiUnit, *mut AbiError) -> AbiStatus;

#[repr(C)]
struct ServiceVTable {
    header: ObjectHeader,
    transform: Transform,
    sum: Sum,
    join: Join,
    panics: Panics,
}

static SERVICE_VTABLE: ServiceVTable = ServiceVTable {
    header: ObjectHeader::for_type::<Service>(size_of::<ServiceVTable>(), 1, "prefix@1.0"),
    transform,
    sum,
    join,
    panics,
};
static SHORT_VTABLE: ObjectHeader =
    ObjectHeader::for_type::<Service>(size_of::<ObjectHeader>(), 1, "prefix@1.0");

// SAFETY: DOC-12 §7，data来自Service的Object，拥有输入和输出槽遵守固定签名。
unsafe extern "C" fn transform(
    data: *mut c_void,
    value: AbiBuf,
    out: *mut AbiBuf,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §7，Service对象在调用期间有效。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §5、§7，先接管输入，再组合服务前缀并交出拥有结果。
    unsafe {
        boundary(env, out, err, || {
            let value = Buf::from_raw(value, env);
            let mut result = object_data::<Service>(data).prefix.clone();
            result.extend(value.into_vec()?);
            Buf::new(result, env).map(Buf::into_raw)
        })
    }
}
// SAFETY: DOC-12 §7，data对应Service，结果槽是i64。
unsafe extern "C" fn sum(
    data: *mut c_void,
    left: i64,
    right: i64,
    out: *mut i64,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §7，公共前缀在调用期间有效。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §7，固定参数布局对应此服务方法。
    unsafe {
        boundary(env, out, err, || {
            Ok(left + right + object_data::<Service>(data).prefix.len() as i64)
        })
    }
}
// SAFETY: DOC-12 §5、§7，两个缓冲的所有权均在进入函数时转交。
unsafe extern "C" fn join(
    data: *mut c_void,
    left: AbiBuf,
    right: AbiBuf,
    out: *mut AbiBuf,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §7，公共前缀在调用期间有效。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §5，先接管两个拥有输入，再执行UTF-8转换。
    unsafe {
        boundary(env, out, err, || {
            let left = Buf::from_raw(left, env);
            let right = Buf::from_raw(right, env);
            let result = left.into_string()? + &right.into_string()?;
            Buf::new(result.into_bytes(), env).map(Buf::into_raw)
        })
    }
}
// SAFETY: DOC-12 §7，data对应Service，结果槽对应单位结果。
unsafe extern "C" fn panics(data: *mut c_void, out: *mut AbiUnit, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §7，公共前缀在调用期间有效。
    let env = unsafe { object_environment(data) };
    // SAFETY: DOC-12 §6，服务内panic完整地位于边界之内。
    unsafe { boundary(env, out, err, || panic!("service failure")) }
}

fn service(env: Environment, vtable: *const ObjectHeader, released: Arc<AtomicUsize>) -> Object {
    let value = Arc::new(Service {
        prefix: b"left/".to_vec(),
        env,
        released,
    });
    // SAFETY: DOC-12 §7，调用方从完整静态测试虚表派生指针，首部由for_type<Service>创建。
    unsafe { Object::new(value, vtable, env) }.unwrap()
}

#[test]
fn objects_transfer_values_in_both_directions_and_keep_code_leased_during_drop() {
    let producer = support::environment();
    let consumer = support::environment();
    let released = Arc::new(AtomicUsize::new(0));
    let outgoing = service(
        producer,
        std::ptr::from_ref(&SERVICE_VTABLE).cast(),
        released.clone(),
    );
    // SAFETY: DOC-12 §5、§11，转交outgoing的唯一拥有引用和既有租约。
    let incoming = unsafe { Object::from_raw(outgoing.into_raw(), consumer) };
    let clone = incoming.try_clone().unwrap();
    // SAFETY: DOC-12 §7.2，槽0为Transform，槽1为Sum。
    let transform = unsafe { incoming.typed_slot::<Transform>(0) }.unwrap();
    // SAFETY: DOC-12 §7.2，ServiceVTable的第二方法为Sum。
    let sum = unsafe { incoming.typed_slot::<Sum>(1) }.unwrap();
    let argument = Buf::new(b"right".to_vec(), consumer).unwrap();
    // SAFETY: DOC-12 §5、§7，拥有参数进入服务，结果转交消费者。
    let raw = unsafe {
        consumer.call(|out, err| transform(incoming.data(), argument.into_raw(), out, err))
    }
    .unwrap();
    // SAFETY: DOC-12 §5，transform交出一个拥有缓冲。
    let result = unsafe { Buf::from_raw(raw, consumer) };
    assert_eq!(result.into_vec().unwrap(), b"left/right");
    // SAFETY: DOC-12 §7，固定方法签名与i64输出一致。
    let result = unsafe { consumer.call(|out, err| sum(incoming.data(), 3, 4, out, err)) }.unwrap();
    assert_eq!(result, 12);
    incoming.release().unwrap();
    assert_eq!(
        released.load(Ordering::SeqCst),
        0,
        "另一拥有引用仍保持实现存活"
    );
    clone.release().unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 1);
    let counts = support::counts(producer);
    assert_eq!(counts.active, 0);
    assert_eq!(
        counts.acquired, counts.released,
        "全部跨界拥有引用已归还租约"
    );
    assert_eq!(support::counts(consumer).active, 0);
}

#[test]
fn decoding_failure_releases_every_owned_argument() {
    let producer = support::environment();
    let consumer = support::environment();
    let object = service(
        producer,
        std::ptr::from_ref(&SERVICE_VTABLE).cast(),
        Arc::new(AtomicUsize::new(0)),
    );
    // SAFETY: DOC-12 §7.2，槽2的固定签名是两个拥有字符串输入。
    let join = unsafe { object.typed_slot::<Join>(2) }.unwrap();
    let left = Buf::new(vec![0xff], consumer).unwrap();
    let right = Buf::new(b"retained input".to_vec(), consumer).unwrap();
    // SAFETY: DOC-12 §5，两个有效缓冲转交给解码边界，内容允许是无效UTF-8输入。
    let result: Result<AbiBuf> = unsafe {
        consumer.call(|out, err| join(object.data(), left.into_raw(), right.into_raw(), out, err))
    };
    assert_eq!(result.err().unwrap().code(), 2);
    assert_eq!(
        support::counts(consumer).active,
        0,
        "第二个输入随解码失败一同释放"
    );
    object.release().unwrap();
    assert_eq!(
        support::counts(producer).active,
        0,
        "错误消息与实现对象均已释放"
    );
}

static VECTOR_FREES: AtomicUsize = AtomicUsize::new(0);
static SLICE_FREES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: DOC-12 §5，ptr/len/cap来自下方Vec分配，唯一所有权在此归还。
unsafe extern "C" fn free_vector(
    _: *mut c_void,
    ptr: *mut u8,
    len: u64,
    cap: u64,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §5、§6，在分配方按实际Vec容量释放。
    unsafe {
        boundary_unit(support::host_environment(), err, || {
            drop(Vec::from_raw_parts(ptr, len as usize, cap as usize));
            VECTOR_FREES.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
// SAFETY: DOC-12 §5，ptr/len来自下方Box<[u8]>分配，唯一所有权在此归还。
unsafe extern "C" fn free_slice(
    _: *mut c_void,
    ptr: *mut u8,
    len: u64,
    _: u64,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §5、§6，按原Box切片布局释放分配。
    unsafe {
        boundary_unit(support::host_environment(), err, || {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                ptr,
                len as usize,
            )));
            SLICE_FREES.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[test]
fn vector_and_boxed_slice_buffers_return_to_their_allocation_owners() {
    let first = support::environment();
    let second = support::environment();
    let mut vector = Vec::with_capacity(64);
    vector.extend_from_slice(b"vector-owned");
    let mut vector = ManuallyDrop::new(vector);
    first.acquire(first.library()).unwrap();
    let raw_vector = AbiBuf {
        ptr: vector.as_mut_ptr(),
        len: vector.len() as u64,
        cap: vector.capacity() as u64,
        owner: std::ptr::null_mut(),
        free: Some(free_vector),
        library: first.library(),
    };
    let slice: Box<[u8]> = b"slice-owned".to_vec().into_boxed_slice();
    let len = slice.len() as u64;
    second.acquire(second.library()).unwrap();
    let raw_slice = AbiBuf {
        ptr: Box::into_raw(slice).cast(),
        len,
        cap: len,
        owner: std::ptr::null_mut(),
        free: Some(free_slice),
        library: second.library(),
    };
    // SAFETY: DOC-12 §5，实际分配和对应free函数已配套，租约已取得。
    let vector = unsafe { Buf::from_raw(raw_vector, second) };
    // SAFETY: DOC-12 §5，接管另一分配方交出的Box切片缓冲。
    let slice = unsafe { Buf::from_raw(raw_slice, first) };
    assert_eq!(vector.into_vec().unwrap(), b"vector-owned");
    assert_eq!(slice.into_vec().unwrap(), b"slice-owned");
    assert_eq!(VECTOR_FREES.load(Ordering::SeqCst), 1);
    assert_eq!(SLICE_FREES.load(Ordering::SeqCst), 1);
    assert_eq!(support::counts(first).active, 0);
    assert_eq!(support::counts(second).active, 0);
}

#[derive(Default)]
struct Signal {
    value: Option<u32>,
    waker: Option<Waker>,
}
struct Waiting {
    signal: Arc<Mutex<Signal>>,
    released: Arc<AtomicUsize>,
}
impl Future for Waiting {
    type Output = Result<u32>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut signal = self.signal.lock().unwrap();
        match signal.value.take() {
            Some(value) => Poll::Ready(Ok(value)),
            None => {
                signal.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}
impl Drop for Waiting {
    fn drop(&mut self) {
        self.signal.lock().unwrap().waker.take();
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Notifications(AtomicUsize);
impl Wake for Notifications {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn poll<T>(future: &mut RemoteFuture<T>, notifications: &Arc<Notifications>) -> Poll<Result<T>> {
    let waker = Waker::from(notifications.clone());
    Pin::new(future).poll(&mut Context::from_waker(&waker))
}

#[test]
fn pending_future_is_woken_across_a_thread_and_returns_its_typed_result() {
    let producer = support::environment();
    let consumer = support::environment();
    let signal = Arc::new(Mutex::new(Signal::default()));
    let released = Arc::new(AtomicUsize::new(0));
    let outgoing = make_future(
        Waiting {
            signal: signal.clone(),
            released: released.clone(),
        },
        producer,
    )
    .unwrap();
    // SAFETY: DOC-12 §8，移动一个输出u32的Future及其租约。
    let mut incoming = unsafe { RemoteFuture::<u32>::from_raw(outgoing.into_raw(), consumer) };
    let notifications = Arc::new(Notifications::default());
    assert!(poll(&mut incoming, &notifications).is_pending());
    assert!(
        support::counts(consumer).active > 0,
        "已保存Waker持有消费者代码租约"
    );
    let waker = {
        let mut signal = signal.lock().unwrap();
        signal.value = Some(41);
        signal.waker.take().unwrap()
    };
    std::thread::spawn(move || {
        waker.wake_by_ref();
        let consumed = waker.clone();
        drop(waker);
        consumed.wake();
    })
    .join()
    .unwrap();
    assert_eq!(notifications.0.load(Ordering::SeqCst), 2);
    assert!(matches!(
        poll(&mut incoming, &notifications),
        Poll::Ready(Ok(41))
    ));
    incoming.release().unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(support::counts(producer).active, 0);
    assert_eq!(support::counts(consumer).active, 0);
}

#[test]
fn cancellation_releases_the_future_and_its_saved_waker() {
    let producer = support::environment();
    let consumer = support::environment();
    let signal = Arc::new(Mutex::new(Signal::default()));
    let released = Arc::new(AtomicUsize::new(0));
    let outgoing = make_future(
        Waiting {
            signal: signal.clone(),
            released: released.clone(),
        },
        producer,
    )
    .unwrap();
    // SAFETY: DOC-12 §8，拥有Future的输出类型由Waiting确定。
    let mut incoming = unsafe { RemoteFuture::<u32>::from_raw(outgoing.into_raw(), consumer) };
    let notifications = Arc::new(Notifications::default());
    assert!(poll(&mut incoming, &notifications).is_pending());
    drop(incoming);
    assert!(
        signal.lock().unwrap().waker.is_none(),
        "取消后保存的Waker随Future释放"
    );
    assert_eq!(notifications.0.load(Ordering::SeqCst), 0);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(support::counts(producer).active, 0);
    assert_eq!(support::counts(consumer).active, 0);
}

#[test]
fn service_and_future_panics_return_code_five_and_leave_output_untouched() {
    let env = support::environment();
    let object = service(
        env,
        std::ptr::from_ref(&SERVICE_VTABLE).cast(),
        Arc::new(AtomicUsize::new(0)),
    );
    // SAFETY: DOC-12 §7.2，槽3是单位结果的panic服务方法。
    let call = unsafe { object.typed_slot::<Panics>(3) }.unwrap();
    let mut result = AbiUnit { reserved: 83 };
    let mut error = MaybeUninit::uninit();
    // SAFETY: DOC-12 §5、§7，输出与错误槽有效，object保持代码存活。
    let status = unsafe { call(object.data(), &mut result, error.as_mut_ptr()) };
    assert_eq!(status, 2);
    assert_eq!(result.reserved, 83, "panic路径保持调用方结果槽原状");
    // SAFETY: DOC-12 §6，status2已经初始化错误并转交其所有权。
    let error = unsafe { env.read_error(error.assume_init()) };
    assert_eq!(error.code(), 5);
    object.release().unwrap();
    let mut future = make_future(
        std::future::poll_fn(|_| -> Poll<Result<u32>> { panic!("poll failure") }),
        env,
    )
    .unwrap();
    let result = poll(&mut future, &Arc::new(Notifications::default()));
    assert!(matches!(result, Poll::Ready(Err(error)) if error.code() == 5));
    future.release().unwrap();
    assert_eq!(support::counts(env).active, 0);
}

struct DropFailure;
impl Drop for DropFailure {
    fn drop(&mut self) {
        panic!("resource drop failure");
    }
}
static DROP_VTABLE: ObjectHeader =
    ObjectHeader::for_type::<DropFailure>(size_of::<ObjectHeader>(), 1, "drop@1.0");

#[test]
fn explicit_drop_reports_failure_and_raii_drop_logs_it_after_releasing_the_lease() {
    let env = support::environment();
    let object =
        // SAFETY: DOC-12 §7，DROP_VTABLE与DropFailure具体类型匹配。
        unsafe { Object::new(Arc::new(DropFailure), std::ptr::from_ref(&DROP_VTABLE), env) }
            .unwrap();
    assert_eq!(object.release().err().unwrap().code(), 5);
    let object =
        // SAFETY: DOC-12 §7，另一独立对象使用同一具体类型虚表。
        unsafe { Object::new(Arc::new(DropFailure), std::ptr::from_ref(&DROP_VTABLE), env) }
            .unwrap();
    drop(object);
    assert!(
        support::messages(env)
            .iter()
            .any(|(level, message)| *level == 0 && message.contains("5")),
        "RAII析构故障以Error级别进入ctx0日志"
    );
    assert_eq!(support::counts(env).active, 0);
}

#[test]
fn missing_method_returns_version_error_without_calling_the_provider() {
    let env = support::environment();
    let released = Arc::new(AtomicUsize::new(0));
    let object = service(env, std::ptr::from_ref(&SHORT_VTABLE), released.clone());
    // SAFETY: DOC-12 §7.2，调用方声明所需方法类型，长度检查判定方法缺项。
    let result = unsafe { object.typed_slot::<Transform>(0) };
    assert_eq!(result.err().unwrap().code(), 3);
    object.release().unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(support::counts(env).active, 0);
}

#[test]
fn callbacks_copy_async_payloads_and_exporters_can_thaw() {
    let producer = support::environment();
    let consumer = support::environment();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let target = observed.clone();
    let sync = sync_callback(producer, move |ctx, scope, payload| {
        target.lock().unwrap().push((ctx, scope, payload.to_vec()));
        Ok(())
    })
    .unwrap();
    // SAFETY: DOC-12 §7.4，转交同步回调的拥有引用。
    let sync = unsafe { Object::from_raw(sync.into_raw(), consumer) };
    sync.call_sync(9, 7, b"sync").unwrap();
    sync.release().unwrap();
    let target = observed.clone();
    let asynchronous = async_callback(producer, move |ctx, scope, payload| {
        let target = target.clone();
        async move {
            target.lock().unwrap().push((ctx, scope, payload));
            Ok(())
        }
    })
    .unwrap();
    // SAFETY: DOC-12 §7.4，转交异步回调的拥有引用。
    let asynchronous = unsafe { Object::from_raw(asynchronous.into_raw(), consumer) };
    let mut payload = b"async".to_vec();
    let mut future = asynchronous.call_async(8, 6, &payload).unwrap();
    payload.fill(b'x');
    asynchronous.release().unwrap();
    assert!(matches!(
        poll(&mut future, &Arc::new(Notifications::default())),
        Poll::Ready(Ok(_))
    ));
    future.release().unwrap();
    assert_eq!(
        *observed.lock().unwrap(),
        vec![(9, 7, b"sync".to_vec()), (8, 6, b"async".to_vec())]
    );
    let frozen = Arc::new(AtomicBool::new(false));
    let freeze = frozen.clone();
    let thaw = frozen.clone();
    let exporter = state_exporter(
        producer,
        move || {
            freeze.store(true, Ordering::SeqCst);
            encode(&17_u64)
        },
        move || {
            thaw.store(false, Ordering::SeqCst);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        decode::<u64>(exporter.export().unwrap().as_slice()).unwrap(),
        17
    );
    assert!(frozen.load(Ordering::SeqCst));
    exporter.thaw().unwrap();
    assert!(!frozen.load(Ordering::SeqCst));
    exporter.release().unwrap();
    assert_eq!(support::counts(producer).active, 0);
    assert_eq!(support::counts(consumer).active, 0);
}

#[test]
fn retired_host_consumes_owned_inputs_while_returning_a_context_error() {
    let env = support::environment();
    let released = Arc::new(AtomicUsize::new(0));
    let object = service(
        env,
        std::ptr::from_ref(&SERVICE_VTABLE).cast(),
        released.clone(),
    );
    let result = env.host().provide(38, "prefix@1.0", object, false);
    assert_eq!(result.err().unwrap().code(), 4);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(support::counts(env).active, 0);
}

#[derive(Serialize, Deserialize)]
struct Problem {
    path: String,
    reason: String,
}

#[test]
fn errors_transfer_messages_and_cbor_details_then_release_both_buffers() {
    let producer = support::environment();
    let consumer = support::environment();
    let mut result = 61_u64;
    let mut error = MaybeUninit::uninit();
    // SAFETY: DOC-12 §5、§6，结果和错误槽有效，闭包交出业务错误。
    let status = unsafe {
        boundary(producer, &mut result, error.as_mut_ptr(), || {
            Err(
                Error::new(1007, "configuration rejected").with_detail(encode(&Problem {
                    path: "config.interval".into(),
                    reason: "positive value required".into(),
                })?),
            )
        })
    };
    assert_eq!(status, 1);
    assert_eq!(result, 61);
    assert_eq!(
        support::counts(producer).active,
        2,
        "消息和详情各自保持源侧释放函数有效"
    );
    // SAFETY: DOC-12 §6，失败状态交出初始化的拥有错误。
    let error = unsafe { consumer.read_error(error.assume_init()) };
    assert_eq!(error.code(), 1007);
    assert_eq!(error.message(), "configuration rejected");
    let problem: Problem = decode(error.detail()).unwrap();
    assert_eq!(problem.path, "config.interval");
    assert_eq!(problem.reason, "positive value required");
    assert_eq!(support::counts(producer).active, 0);
}

#[derive(Serialize, Deserialize)]
struct FixtureConfig {
    count: usize,
}
static ENTRY_LIBRARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static APPLIED: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct ExtendedComponent {
    component: ComponentV1,
    capability: u64,
}

#[repr(C)]
struct ExtendedDescriptor {
    descriptor: PluginV1,
    base: *const PluginV1,
    components: Box<[ExtendedComponent; 2]>,
}

fn entry_environment() -> Environment {
    // SAFETY: DOC-12 §3，fixture的入口始终接收support::HOST，库在描述符释放前保留租约。
    unsafe { Environment::new(&support::HOST, ENTRY_LIBRARY.load(Ordering::SeqCst)) }
}

// SAFETY: DOC-12 §3，宿主表有效，out和err是入口约定的输出槽。
unsafe extern "C" fn fixture_entry(
    host: *const HostV1,
    library: LibraryId,
    out: *mut *const PluginV1,
    err: *mut AbiError,
) -> AbiStatus {
    // SAFETY: DOC-12 §3、§10，builder保有元数据，扩展记录复制借用字段并拥有完整数组存储。
    unsafe {
        entry_boundary(host, library, out, err, |_| {
            ENTRY_LIBRARY.store(library, Ordering::SeqCst);
            let definitions = ["configured", "configured-extra"]
                .into_iter()
                .map(|name| ComponentDefinition {
                    name: name.into(),
                    inject: vec![Dependency {
                        interface: "store@1.0".into(),
                        required: true,
                    }],
                    provides: vec!["counter@1.0".into()],
                    listens: vec!["message@1.0".into()],
                    state_schema: "fixture.state".into(),
                    state_version_min: 1,
                    state_version_max: 2,
                    apply: fixture_apply,
                    resolve: fixture_resolve,
                })
                .collect();
            let base = PluginDescriptor::new(
                PluginMetadata {
                    name: "descriptor-fixture".into(),
                    version: "1.0.0".into(),
                    sdk_version: "0.1.0".into(),
                    build_info: "test process".into(),
                    panic_unwind: true,
                },
                definitions,
            )
            .into_raw();
            let mut components = Box::new([
                ExtendedComponent {
                    component: std::ptr::read(component_at(&*base, 0)?),
                    capability: 11,
                },
                ExtendedComponent {
                    component: std::ptr::read(component_at(&*base, 1)?),
                    capability: 22,
                },
            ]);
            for record in components.iter_mut() {
                record.component.size = size_of::<ExtendedComponent>() as u64;
            }
            let mut descriptor = base.read();
            descriptor.abi_minor = 1;
            descriptor.release = release_extended;
            let owner = Box::into_raw(Box::new(ExtendedDescriptor {
                descriptor,
                base,
                components,
            }));
            // DOC-12 §10：所有Box进入最终拥有存储后，从该存储建立整张记录表的指针。
            (*owner).descriptor.components = (*owner).components.as_ptr().cast();
            Ok(owner.cast())
        })
    }
}

// SAFETY: DOC-12 §10，指针来自fixture_entry的ExtendedDescriptor，唯一所有权转入。
unsafe extern "C" fn release_extended(raw: *const PluginV1, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§10，repr(C)首部恢复拥有存储，复制释放函数后归还底层描述符。
    unsafe {
        boundary_unit(support::host_environment(), err, || {
            let storage = Box::from_raw(raw.cast_mut().cast::<ExtendedDescriptor>());
            let base = storage.base;
            let release = (*base).release;
            let result = support::host_environment().call_unit(|err| release(base, err));
            drop(storage);
            result
        })
    }
}
// SAFETY: DOC-12 §10，config是调用期CBOR借用，err为有效错误槽。
unsafe extern "C" fn fixture_resolve(config: AbiBytes, err: *mut AbiError) -> AbiStatus {
    // SAFETY: DOC-12 §5、§6，配置读取和验证位于边界内。
    unsafe {
        boundary_unit(entry_environment(), err, || {
            let config: FixtureConfig = decode(config.as_slice())?;
            if config.count == 0 {
                return Err(Error::new(2, "count must be positive"));
            }
            Ok(())
        })
    }
}
// SAFETY: DOC-12 §10，配置借用覆盖入口调用，out是单位Future输出槽。
unsafe extern "C" fn fixture_apply(
    _: u64,
    config: AbiBytes,
    out: *mut AbiFuture,
    err: *mut AbiError,
) -> AbiStatus {
    let env = entry_environment();
    // SAFETY: DOC-12 §5、§8，入口返回前解析配置，Future捕获拥有值。
    unsafe {
        boundary(env, out, err, || {
            let config: FixtureConfig = decode(config.as_slice())?;
            make_future(
                async move {
                    APPLIED.fetch_add(config.count, Ordering::SeqCst);
                    Ok(AbiUnit::default())
                },
                env,
            )
            .map(RemoteFuture::into_raw)
        })
    }
}

#[test]
fn descriptor_storage_survives_entry_and_drives_resolve_then_apply() {
    let env = support::environment();
    env.acquire(env.library()).unwrap();
    // SAFETY: DOC-12 §3，fixture入口代码位于本测试进程且遵守描述符契约。
    let entry = unsafe { PluginEntry::new(fixture_entry) };
    let raw =
        // SAFETY: DOC-12 §3，宿主表和输出槽有效，调用者持有描述符所属库租约。
        unsafe { env.call(|out, err| entry.function()(&support::HOST, env.library(), out, err)) }
            .unwrap();
    // SAFETY: DOC-12 §10，成功入口返回稳定存储的完整描述符。
    let descriptor = unsafe { &*raw };
    assert_eq!(
        // SAFETY: DOC-12 §10，描述符拥有名称存储，借用覆盖本次读取。
        unsafe { descriptor.name.as_str() }.unwrap(),
        "descriptor-fixture"
    );
    assert_eq!(descriptor.component_count, 2);
    // SAFETY: DOC-12 §10，描述符持有完整扩展记录表，helper按size定位首条记录。
    let component = unsafe { component_at(descriptor, 0) }.unwrap();
    // SAFETY: DOC-12 §10，组件名称存储由描述符持有。
    assert_eq!(unsafe { component.name.as_str() }.unwrap(), "configured");
    // SAFETY: DOC-12 §10，注入数组包含builder声明的一个元素。
    let dependency = unsafe { &*component.inject };
    assert_eq!(
        // SAFETY: DOC-12 §10，依赖名称存储由描述符持有。
        unsafe { dependency.interface.as_str() }.unwrap(),
        "store@1.0"
    );
    // SAFETY: DOC-12 §10，第二条记录跟随首条的完整size，描述符保持整张表存活。
    let component = unsafe { component_at(descriptor, 1) }.unwrap();
    assert_eq!(
        // SAFETY: DOC-12 §10，第二条记录的名称由底层描述符持有。
        unsafe { component.name.as_str() }.unwrap(),
        "configured-extra"
    );
    let config = encode(&FixtureConfig { count: 7 }).unwrap();
    // SAFETY: DOC-12 §10，resolve签名和配置借用满足描述符契约。
    unsafe { env.call_unit(|err| (component.resolve)(AbiBytes::new(&config), err)) }.unwrap();
    let raw_future =
        // SAFETY: DOC-12 §10，apply交出单位结果Future。
        unsafe { env.call(|out, err| (component.apply)(1, AbiBytes::new(&config), out, err)) }
            .unwrap();
    let mut future =
        // SAFETY: DOC-12 §8，apply结果为单位Future并附带租约。
        unsafe { RemoteFuture::<AbiUnit>::from_raw(raw_future, support::host_environment()) };
    assert_eq!(APPLIED.load(Ordering::SeqCst), 0);
    assert!(matches!(
        poll(&mut future, &Arc::new(Notifications::default())),
        Poll::Ready(Ok(_))
    ));
    assert_eq!(APPLIED.load(Ordering::SeqCst), 7);
    future.release().unwrap();
    let release = descriptor.release;
    // SAFETY: DOC-12 §10，最后一次描述符访问后归还其唯一所有权。
    unsafe { env.call_unit(|err| release(raw, err)) }.unwrap();
    env.release(env.library()).unwrap();
    assert_eq!(support::counts(env).active, 0);
}
