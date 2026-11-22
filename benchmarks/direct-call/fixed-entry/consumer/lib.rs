#[path = "../abi.rs"]
mod abi;

use abi::{Bootstrap, Counter, Entry, Inherited};
use cordis_sdk::{
    __private::abi::{AbiBytes, AbiStr},
    prelude::*,
};
use libloading::Library;
use serde::{Deserialize, Serialize};
use std::{
    ffi::c_void, fs::File, hint::black_box, io::Write, mem::size_of, path::PathBuf, ptr::NonNull,
    time::Instant,
};

fn error(problem: impl std::fmt::Display) -> Error {
    Error::new(1, problem.to_string())
}

struct Image<'a> {
    table: &'a Entry,
    instance: NonNull<c_void>,
    method: i32,
}

impl Drop for Image<'_> {
    fn drop(&mut self) {
        // SAFETY: Image 拥有 load 返回的实例，并借用提供方库直到 close 返回。
        unsafe { (self.table.close)(self.instance.as_ptr()) }
    }
}

struct InputOwner {
    data: Vec<u8>,
    released: Arc<std::sync::atomic::AtomicBool>,
}

unsafe extern "C" fn release_input(owner: *mut c_void) {
    // SAFETY: 所有者由 load_input 以 Box::into_raw 移交；Inherited 的析构负责消费。
    let owner = unsafe { Box::from_raw(owner.cast::<InputOwner>()) };
    owner
        .released
        .store(true, std::sync::atomic::Ordering::Release);
}

fn load_input(table: &Entry, version: u32, initial: u64) -> Result<(i32, *mut c_void)> {
    let mut data = Vec::new();
    ciborium::into_writer(&("benchmark.counter.state", version, initial), &mut data)
        .map_err(error)?;
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let owner = Box::new(InputOwner {
        data,
        released: released.clone(),
    });
    let input = Inherited {
        bytes: AbiBytes::new(&owner.data),
        owner: Box::into_raw(owner).cast(),
        release: release_input,
    };
    let mut instance = std::ptr::null_mut();
    // SAFETY: 输入缓冲属于刚移交的 Box，instance 是本次调用的独占输出槽。
    let status = unsafe { (table.load)(input, &mut instance) };
    if !released.load(std::sync::atomic::Ordering::Acquire) {
        return Err(error("加载调用返回时，提供方仍持有继承输入"));
    }
    Ok((status, instance))
}

impl<'a> Image<'a> {
    fn open(library: &'a Library, value: u64) -> Result<Self> {
        // SAFETY: 此基准只装载由同批提供方构建并由配置指定的固定入口。
        let entry =
            unsafe { library.get::<Bootstrap>(b"cordis_benchmark_entry_v2") }.map_err(error)?;
        // SAFETY: 此版本化符号返回具有静态寿命的 C 表，library 覆盖其借用期。
        let raw = unsafe { entry() };
        if raw.is_null() || unsafe { (*raw).major != 2 || (*raw).size < size_of::<Entry>() } {
            return Err(error("提供方的 ABI 主版本或描述符大小与消费方要求不符"));
        }
        // SAFETY: 版本及长度已经从公共前缀取得。
        let table = unsafe { &*raw };
        let bind = |signature| unsafe {
            (table.bind)(
                AbiStr::new("benchmark.counter@1"),
                AbiStr::new("value"),
                AbiStr::new(signature),
            )
        };
        if bind("sync()->string") != -3 {
            return Err(error("签名冲突应在绑定阶段返回 -3"));
        }
        let method = bind("sync()->u64");
        if method < 0 {
            return Err(error("按完整业务方法身份绑定失败"));
        }
        let (rejected, unused) = load_input(table, 2, value)?;
        if rejected != -2 || !unused.is_null() {
            return Err(error("不相容的状态模式应拒绝加载"));
        }
        let (status, instance) = load_input(table, 1, value)?;
        if status != 0 {
            return Err(error(format!("加载失败：{status}")));
        }
        Ok(Self {
            table,
            instance: NonNull::new(instance).ok_or_else(|| error("缺少实例句柄"))?,
            method,
        })
    }

    fn value(&self) -> Result<u64> {
        // 空位置参数的规范 CBOR 编码可以跨调用复用，u64 的最长结果占 9 字节。
        let mut result = [0_u8; 9];
        // SAFETY: 实例的创建与释放由 Image 决定，输入和输出缓冲覆盖同步调用。
        let size = unsafe {
            (self.table.invoke)(
                self.instance.as_ptr(),
                self.method,
                AbiBytes::new(&[0x80]),
                result.as_mut_ptr(),
                result.len(),
            )
        };
        let result = usize::try_from(size)
            .ok()
            .and_then(|size| result.get(..size))
            .ok_or_else(|| error(format!("固定入口交付的结果长度为 {size}")))?;
        ciborium::from_reader(result).map_err(error)
    }
}

#[derive(Deserialize)]
struct Settings {
    provider: PathBuf,
    output: PathBuf,
    initial: u64,
    rounds: usize,
    warmup: usize,
    iterations: usize,
    latency_samples: usize,
}

#[derive(Serialize)]
struct Sample {
    pair: usize,
    path: &'static str,
    calls: usize,
    elapsed_ns: u128,
    latency_ns: Vec<u128>,
}

fn measure(
    pair: usize,
    path: &'static str,
    config: &Settings,
    call: impl Fn() -> Result<u64>,
) -> Result<Sample> {
    if call()? != config.initial {
        return Err(error("提供方返回的业务状态与部署输入不同"));
    }
    for _ in 0..config.warmup {
        black_box(call()?);
    }
    let start = Instant::now();
    for _ in 0..config.iterations {
        black_box(call()?);
    }
    let elapsed_ns = start.elapsed().as_nanos();
    let mut latency_ns = Vec::with_capacity(config.latency_samples);
    for _ in 0..config.latency_samples {
        let start = Instant::now();
        black_box(call()?);
        latency_ns.push(start.elapsed().as_nanos());
    }
    Ok(Sample {
        pair,
        path,
        calls: config.iterations,
        elapsed_ns,
        latency_ns,
    })
}

struct Probe;
impl Component for Probe {
    const NAME: &'static str = "probe";
    type Config = Settings;
    fn inject() -> Inject {
        Inject::new().with::<dyn Counter>()
    }
    async fn apply(ctx: Context, config: Settings) -> Result<()> {
        let typed = ctx.get::<dyn Counter>()?;
        if !typed.clone_object()?.has_typed_methods() {
            return Err(error("基线需要同宿主类型化服务"));
        }
        // SAFETY: 独立提供方路径来自测量部署；本库的关闭顺序由 Image 的借用约束。
        let library = unsafe { Library::new(&config.provider) }.map_err(error)?;
        let fixed = Image::open(&library, config.initial)?;
        let mut samples = Vec::new();
        for pair in 0..=config.rounds {
            for fixed_first in [pair % 2 == 0, pair % 2 != 0] {
                samples.push(if fixed_first {
                    measure(pair, "fixed-cbor", &config, || fixed.value())?
                } else {
                    measure(pair, "typed-v1", &config, || typed.value())?
                });
            }
        }
        let mut output = std::io::BufWriter::new(File::create_new(&config.output).map_err(error)?);
        serde_json::to_writer(&mut output, &samples).map_err(error)?;
        output.flush().map_err(error)
    }
}

cordis_sdk::export_plugin!(
    name = "fixed-entry-consumer",
    version = env!("CARGO_PKG_VERSION"),
    components = [Probe]
);
