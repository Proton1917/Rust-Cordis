//! DOC-15 §7：真实生成接口经过 SDK 的类型化入口、通用入口及资源方法。

#[path = "../../../../../cordis-sdk/tests/support/host.rs"]
#[allow(dead_code)]
mod host;

use crate::{
    kv,
    types::{
        cordis__types::{FutureU32, StreamU32},
        *,
    },
};
use cordis_abi::{CborValue, Object, testing::environment};
use cordis_sdk::prelude::*;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicU32, AtomicUsize, Ordering},
    },
};

struct CounterValue {
    value: AtomicU32,
    released: Arc<AtomicUsize>,
}
impl Counter for CounterValue {
    fn read(&self) -> Result<u32> {
        Ok(self.value.load(Ordering::SeqCst))
    }
    fn add(&self, delta: u32) -> Result<u32> {
        Ok(self.value.fetch_add(delta, Ordering::SeqCst) + delta)
    }
    fn later(&self) -> BoxFuture<'_, Result<u32>> {
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            self.read()
        })
    }
}
impl Drop for CounterValue {
    fn drop(&mut self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

struct FutureValue(u32, Arc<AtomicUsize>);
impl FutureU32 for FutureValue {
    fn resolve(&self) -> BoxFuture<'_, Result<u32>> {
        Box::pin(async { Ok(self.0) })
    }
}
impl Drop for FutureValue {
    fn drop(&mut self) {
        self.1.fetch_add(1, Ordering::SeqCst);
    }
}
struct SequenceValue(Mutex<VecDeque<u32>>, Arc<AtomicUsize>);
impl StreamU32 for SequenceValue {
    fn next(&self) -> BoxFuture<'_, Result<Option<u32>>> {
        Box::pin(async { Ok(self.0.lock().unwrap().pop_front()) })
    }
}
impl Drop for SequenceValue {
    fn drop(&mut self) {
        self.1.fetch_add(1, Ordering::SeqCst);
    }
}

struct LaboratoryValue {
    context: Context,
    released: Arc<AtomicUsize>,
}
impl Laboratory for LaboratoryValue {
    fn new__counter(&self, initial: u32) -> Result<Service<dyn Counter>> {
        self.context.service(Arc::new(CounterValue {
            value: AtomicU32::new(initial),
            released: self.released.clone(),
        }) as Arc<dyn Counter>)
    }
    fn transform(&self, mut input: Sample) -> Result<Sample> {
        input.label.push_str(":rust");
        input.coordinates.0 += 1;
        input.choice = Choice::Message("processed".into());
        Ok(input)
    }
    fn choose(&self, value: Choice) -> Result<Choice> {
        Ok(match value {
            Choice::Empty(()) => Choice::Number(17),
            Choice::Number(value) => Choice::Number(value + 1),
            Choice::Message(value) => Choice::Message(value.to_uppercase()),
        })
    }
    fn nested_values(
        &self,
        mut values: Vec<Option<Option<u32>>>,
    ) -> Result<Vec<Option<Option<u32>>>> {
        values.push(Some(None));
        Ok(values)
    }
    fn nested_maybe(
        &self,
        values: Option<Vec<Option<Option<u32>>>>,
    ) -> Result<Option<Vec<Option<Option<u32>>>>> {
        values.map(|values| self.nested_values(values)).transpose()
    }
    fn relay(&self, value: Bundle) -> Result<Bundle> {
        value.main.add(2)?;
        Ok(value)
    }
    fn inspect(&self, value: Loan<'_>) -> Result<u32> {
        value.counter.read()
    }
    fn read_later(&self, value: Service<dyn Counter>) -> BoxFuture<'_, Result<u32>> {
        Box::pin(async move { value.later().await })
    }
    fn eventual_value(&self, value: u32) -> Result<Service<dyn FutureU32>> {
        self.context
            .service(Arc::new(FutureValue(value, self.released.clone())) as Arc<dyn FutureU32>)
    }
    fn sequence_value(&self) -> Result<Service<dyn StreamU32>> {
        self.context.service(Arc::new(SequenceValue(
            Mutex::new(VecDeque::from([3, 5])),
            self.released.clone(),
        )) as Arc<dyn StreamU32>)
    }
    fn reject(&self) -> Result<u32> {
        Err(Error::new(1070, "laboratory rejected").with_detail(cordis_sdk::encode(&"details")?))
    }
}

#[derive(Default)]
struct KvValue(Mutex<BTreeMap<String, Vec<u8>>>);
impl kv::Kv for KvValue {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), value);
        Ok(())
    }
    fn keys(&self) -> Result<Vec<String>> {
        Ok(self.0.lock().unwrap().keys().cloned().collect())
    }
    fn flush(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

struct Relay(Object);
impl cordis_abi::GenericService for Relay {
    fn invoke(&self, method: &str, args: &[u8]) -> Result<cordis_abi::Buf> {
        self.0.invoke(method, args)
    }
    fn invoke_async(&self, method: &str, args: &[u8]) -> Result<cordis_abi::InvokeFuture> {
        self.0.invoke_async(method, args)
    }
}

fn generic<I: Interface + ?Sized>(service: &Service<I>) -> Result<Service<I>> {
    let object = service.clone_object()?;
    let proxy = cordis_abi::generic_object(
        object.interface()?.into(),
        object.method_names()?,
        Arc::new(Relay(object)),
        environment(9),
    )?;
    Service::from_object(proxy)
}

fn sample() -> Sample {
    Sample {
        label: "数据".into(),
        marker: '𝄞',
        count: u64::MAX,
        delta: i64::MIN,
        ratio: 1.5,
        active: true,
        bytes: vec![0, 128, 255],
        pairs: BTreeMap::from([("k".into(), vec![7, 8])]),
        coordinates: (-3, 0.5),
        window: [4; 40],
        choice: Choice::Empty(()),
        state: State::Ready,
        permissions: Permissions {
            read_value: true,
            write_value: false,
        },
        nested: Some(None),
        options: vec![None, Some(None), Some(Some(9))],
        outcome: Err("business".into()),
    }
}

fn field<'a>(value: &'a CborValue, name: &str) -> &'a CborValue {
    value
        .as_map()
        .unwrap()
        .iter()
        .find_map(|(key, value)| (key.as_text() == Some(name)).then_some(value))
        .unwrap()
}

/// 独立期望值覆盖数组、分支、64 位精度与 Some(None) 标签。
fn inspect_wire() -> Result<()> {
    let bytes = cordis_sdk::encode(&sample())?;
    let value = cordis_abi::decode_value(&bytes)?;
    assert_eq!(
        field(&value, "bytes"),
        &CborValue::Array(vec![0.into(), 128.into(), 255.into()])
    );
    assert_eq!(
        field(&value, "nested"),
        &CborValue::Tag(60001, Box::new(CborValue::Null))
    );
    assert_eq!(
        field(&value, "choice"),
        &CborValue::Map(vec![("empty".into(), CborValue::Null)])
    );
    assert_eq!(
        field(&value, "outcome"),
        &CborValue::Map(vec![("err".into(), "business".into())])
    );
    let snapshot = kv::Snapshot {
        entries: BTreeMap::from([("a".into(), vec![1, 2])]),
    };
    let value = cordis_abi::to_value(&snapshot)?;
    assert_eq!(
        field(field(&value, "entries"), "a"),
        &CborValue::Array(vec![1.into(), 2.into()])
    );
    std::fs::write(
        concat!(env!("CORDIS_IDL_GENERATED"), "/rust-sample.cbor"),
        bytes,
    )
    .unwrap();
    Ok(())
}

async fn service_behavior(service: Service<dyn Laboratory>, released: &AtomicUsize) -> Result<()> {
    let output = service.transform(sample())?;
    assert_eq!(output.label, "数据:rust");
    assert_eq!(output.coordinates, (-2, 0.5));
    assert_eq!(output.nested, Some(None));
    assert_eq!(output.options, [None, Some(None), Some(Some(9))]);
    assert_eq!(output.window, [4; 40]);
    assert_eq!(service.choose(Choice::Empty(()))?, Choice::Number(17));
    assert_eq!(
        service.nested_values(vec![None, Some(None)])?,
        [None, Some(None), Some(None)]
    );
    assert_eq!(service.nested_maybe(None)?, None);
    assert_eq!(
        service.nested_maybe(Some(vec![Some(Some(6))]))?,
        Some(vec![Some(Some(6)), Some(None)])
    );
    let initial = released.load(Ordering::SeqCst);
    let counter = service.new__counter(7)?;
    assert_eq!(
        service.inspect(Loan {
            counter: counter.borrow()
        })?,
        7
    );
    assert_eq!(counter.read()?, 7);
    let bundle = service.relay(Bundle {
        main: counter.clone(),
        extras: vec![counter.clone()],
        optional: Some(counter.clone()),
    })?;
    assert_eq!(bundle.extras[0].read()?, 9);
    assert_eq!(bundle.optional.as_ref().unwrap().read()?, 9);
    drop((bundle, counter));
    assert_eq!(released.load(Ordering::SeqCst), initial + 1);
    let future = service.eventual_value(41)?;
    assert_eq!(future.resolve().await?, 41);
    drop(future);
    let stream = service.sequence_value()?;
    assert_eq!(stream.next().await?, Some(3));
    assert_eq!(stream.next().await?, Some(5));
    assert_eq!(stream.next().await?, None);
    drop(stream);
    let mut pending = service.read_later(service.new__counter(12)?);
    assert!(futures_util::poll!(pending.as_mut()).is_pending());
    drop(pending);
    assert_eq!(released.load(Ordering::SeqCst), initial + 4);
    let error = service.reject().unwrap_err();
    assert_eq!(error.code(), 1070);
    assert_eq!(cordis_sdk::decode::<String>(error.detail())?, "details");
    Ok(())
}

#[tokio::test]
async fn generated_bindings_execute_through_real_sdk() -> Result<()> {
    inspect_wire()?;
    let host = Arc::new(host::Loopback::new());
    cordis_abi::testing::install(host.clone())?;
    let id = host.context(Vec::new(), Vec::new(), None);
    let context = Context::new(environment(9), id);
    let released = Arc::new(AtomicUsize::new(0));
    let value = Arc::new(LaboratoryValue {
        context: context.clone(),
        released: released.clone(),
    });
    let typed = context.service(value as Arc<dyn Laboratory>)?;
    let routed = generic(&typed)?;
    service_behavior(typed, &released).await?;
    service_behavior(routed, &released).await?;
    let typed = context.service(Arc::new(KvValue::default()) as Arc<dyn kv::Kv>)?;
    let routed = generic(&typed)?;
    for value in [typed, routed] {
        assert_eq!(value.get("missing")?, None);
        value.put("first", vec![0, 255])?;
        assert_eq!(value.get("first")?, Some(vec![0, 255]));
        assert_eq!(value.keys()?, ["first"]);
        value.flush().await?;
    }
    let notifications = Arc::new(Mutex::new(Vec::new()));
    let observed = notifications.clone();
    context.on::<kv::Changed, _>(move |value| {
        observed.lock().unwrap().push(value.key);
        Ok(())
    })?;
    context.emit(kv::Changed {
        key: "first".into(),
    })?;
    assert_eq!(*notifications.lock().unwrap(), ["first"]);
    host.dispose(id).await?;
    assert_eq!(host.resources(), 0);
    assert!(host.leases().values().all(|count| *count == 0));
    Ok(())
}

/// 脚本进程的实际 CBOR 输出进入 Rust 数据类型，检查独立规定的变换结果。
#[test]
fn script_results_preserve_wit_values_in_rust() -> Result<()> {
    for (language, coordinate) in [("python", -1), ("typescript", 0)] {
        let path = format!("{}/{language}-sample.cbor", env!("CORDIS_IDL_GENERATED"));
        let bytes = std::fs::read(path).unwrap();
        let value: Sample = cordis_sdk::decode(&bytes)?;
        assert_eq!(value.label, format!("数据:{language}"));
        assert_eq!(value.coordinates, (coordinate, 0.5));
        assert_eq!(value.marker, '𝄞');
        assert_eq!(value.count, u64::MAX);
        assert_eq!(value.delta, i64::MIN);
        assert_eq!(value.bytes, [0, 128, 255]);
        assert_eq!(value.pairs["k"], [7, 8]);
        assert_eq!(value.nested, Some(None));
        assert_eq!(value.options, [None, Some(None), Some(Some(9))]);
        assert_eq!(value.outcome, Err("business".into()));
        assert_eq!(value.choice, Choice::Empty(()));
        assert_eq!(value.window, [4; 40]);
    }
    Ok(())
}
