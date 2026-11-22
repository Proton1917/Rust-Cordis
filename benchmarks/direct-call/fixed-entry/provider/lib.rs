#[path = "../abi.rs"]
mod abi;

use abi::{Counter, Entry, Inherited};
use cordis_sdk::{
    __private::abi::{AbiBytes, AbiStr},
    prelude::*,
};
use std::{
    ffi::c_void,
    io::Cursor,
    mem::size_of,
    sync::atomic::{AtomicU64, Ordering},
};

struct Store(AtomicU64);

#[cordis_sdk::interface]
impl Counter for Store {
    fn value(&self) -> Result<u64> {
        Ok(self.0.load(Ordering::Relaxed))
    }
}

#[derive(serde::Deserialize)]
struct Initial {
    value: u64,
}

struct Provider;
impl Component for Provider {
    const NAME: &'static str = "provider";
    type Config = Initial;
    fn provides() -> Provides {
        Provides::new().with::<dyn Counter>()
    }
    async fn apply(ctx: Context, initial: Initial) -> Result<()> {
        ctx.provide::<dyn Counter>(Arc::new(Store(AtomicU64::new(initial.value))))
    }
}

cordis_sdk::export_plugin!(
    name = "fixed-entry-provider",
    version = env!("CARGO_PKG_VERSION"),
    components = [Provider]
);

unsafe extern "C" fn load(input: Inherited, out: *mut *mut c_void) -> i32 {
    // SAFETY: 加载方传入独占输出槽及由 input 拥有的有效字节。
    let bytes = unsafe { input.bytes.as_slice() };
    let mut reader = Cursor::new(bytes);
    let Ok((schema, version, value)) = ciborium::from_reader::<(String, u32, u64), _>(&mut reader)
    else {
        return -2;
    };
    if schema != "benchmark.counter.state"
        || version != 1
        || reader.position() != bytes.len() as u64
    {
        return -2;
    }
    // SAFETY: 此句将新实例的唯一释放责任交给加载方。
    unsafe { out.write(Box::into_raw(Box::new(Store(AtomicU64::new(value)))).cast()) };
    0
}

unsafe extern "C" fn bind(interface: AbiStr, method: AbiStr, signature: AbiStr) -> i32 {
    // SAFETY: 消费方在绑定调用期间持有三个 UTF-8 字符串。
    match unsafe { (interface.as_str(), method.as_str(), signature.as_str()) } {
        (Ok("benchmark.counter@1"), Ok("value"), Ok("sync()->u64")) => 1,
        _ => -3,
    }
}

unsafe extern "C" fn invoke(
    instance: *const c_void,
    method: i32,
    arguments: AbiBytes,
    out: *mut u8,
    capacity: usize,
) -> isize {
    if method != 1 {
        return -3;
    }
    // SAFETY: 调用期由消费方持有实例租约、输入及独占结果切片。
    let (store, bytes, output) = unsafe {
        (
            &*instance.cast::<Store>(),
            arguments.as_slice(),
            std::slice::from_raw_parts_mut(out, capacity),
        )
    };
    let mut reader = Cursor::new(bytes);
    if ciborium::from_reader::<[u64; 0], _>(&mut reader).is_err()
        || reader.position() != bytes.len() as u64
    {
        return -2;
    }
    let mut writer = Cursor::new(output);
    match store.value().and_then(|value| {
        ciborium::into_writer(&value, &mut writer).map_err(|error| Error::new(8, error.to_string()))
    }) {
        Ok(()) => writer.position() as isize,
        Err(_) => -8,
    }
}

unsafe extern "C" fn close(instance: *mut c_void) {
    // SAFETY: 加载成功产生的实例由唯一消费方关闭，来源库仍在内存中。
    drop(unsafe { Box::from_raw(instance.cast::<Store>()) });
}

static ENTRY: Entry = Entry {
    major: 2,
    size: size_of::<Entry>(),
    load,
    bind,
    invoke,
    close,
};

#[unsafe(no_mangle)]
pub extern "C" fn cordis_benchmark_entry_v2() -> *const Entry {
    &ENTRY
}
