use cordis_abi::{CborValue, from_value, to_value};
use cordis_up::{Limits, decode_value, payload::Apply};
use serde::Serialize;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    env, fs,
    hint::black_box,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

static REQUESTS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

struct Allocator;

impl Allocator {
    fn record(pointer: *mut u8, bytes: usize) -> *mut u8 {
        if !pointer.is_null() {
            REQUESTS.fetch_add(1, Relaxed);
            BYTES.fetch_add(bytes, Relaxed);
        }
        pointer
    }
}

// SAFETY: 布局、指针及释放均交给 System，计数只更新独立的原子整数。
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::record(unsafe { System.alloc(layout) }, layout.size())
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::record(unsafe { System.alloc_zeroed(layout) }, layout.size())
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        Self::record(unsafe { System.realloc(pointer, layout, size) }, size)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[derive(Serialize)]
struct Allocations {
    requests: usize,
    requested_bytes: usize,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn count(
    input: &CborValue,
    decode: impl FnOnce() -> cordis_abi::Result<Apply>,
) -> Result<Allocations> {
    let requests = REQUESTS.load(Relaxed);
    let bytes = BYTES.load(Relaxed);
    let decoded = black_box(decode()?);
    let allocations = Allocations {
        requests: REQUESTS.load(Relaxed) - requests,
        requested_bytes: BYTES.load(Relaxed) - bytes,
    };
    if to_value(&decoded)? != *input {
        return Err("解码得到的 apply 请求与输入有差异".into());
    }
    Ok(allocations)
}

fn main() -> Result<()> {
    let paths = env::args_os().skip(1);
    if paths.len() == 0 {
        return Err("请指定 KV 检查点文件".into());
    }
    for path in paths {
        let path = Path::new(&path);
        let bytes = fs::read(path)?;
        let checkpoint = decode_value(&bytes, &Limits::DEFAULT)?;
        let request: Apply = Apply {
            plugin: "kv".into(),
            component: "kv-store".into(),
            library: Some(1),
            context: 1,
            config: CborValue::Map(Vec::new()),
            inherited: Some(from_value(checkpoint)?),
        };
        let input = to_value(&request)?;
        // 2edc244 的宿主按值传入克隆，现行宿主借用原请求。两路经同一生产解码函数。
        let clone_then_decode = count(&input, || from_value(black_box(input.clone())))?;
        let borrow_then_decode = count(&input, || from_value(black_box(&input)))?;
        println!(
            "{}",
            serde_json::json!({
                "checkpoint": path.file_name().and_then(|name| name.to_str()),
                "checkpoint_bytes": bytes.len(),
                "clone_then_decode": clone_then_decode,
                "borrow_then_decode": borrow_then_decode,
            })
        );
    }
    Ok(())
}
