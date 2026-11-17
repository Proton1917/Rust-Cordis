//! ADR-0055：插件库级 tokio 执行上下文。
//!
//! 插件动态库静态链接自己的一份 tokio，宿主的运行时对它不可见。SDK 在插件握手时
//! 登记 cordis-abi 的执行环境钩子：第一次进入插件代码时创建一个库级运行时，之后每次
//! 边界调用都进入它的上下文，插件里的 `tokio::spawn`、`tokio::time` 与依赖 tokio 的
//! 第三方库因此可以直接使用。描述符释放（库退役）时运行时被关闭，且不会再次创建。

#[cfg(feature = "tokio")]
mod runtime {
    use std::sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::runtime::{Builder, Handle, Runtime};

    /// 运行时本体，释放时取出并在后台关闭。
    static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);
    /// 进入上下文用的句柄；创建失败时为 None。
    static HANDLE: OnceLock<Option<Handle>> = OnceLock::new();
    static RELEASED: AtomicBool = AtomicBool::new(false);

    fn handle() -> Option<&'static Handle> {
        if RELEASED.load(Ordering::Acquire) {
            return HANDLE.get().and_then(Option::as_ref);
        }
        HANDLE.get_or_init(create).as_ref()
    }

    fn create() -> Option<Handle> {
        let built = Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("cordis-plugin-tokio")
            .enable_all()
            .build();
        match built {
            Ok(runtime) => {
                let handle = runtime.handle().clone();
                *RUNTIME.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(runtime);
                Some(handle)
            }
            Err(error) => {
                eprintln!("插件 tokio 运行时创建失败，插件代码将不处于 tokio 上下文：{error}");
                None
            }
        }
    }

    fn enter(operation: &mut dyn FnMut()) {
        match handle() {
            Some(handle) => {
                let _entered = handle.enter();
                operation();
            }
            None => operation(),
        }
    }

    fn release() {
        RELEASED.store(true, Ordering::Release);
        let runtime = RUNTIME
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        if let Some(runtime) = runtime {
            runtime.shutdown_background();
        }
    }

    pub(super) fn install() {
        cordis_abi::ambient::install(cordis_abi::ambient::Ambient { enter, release });
    }

    /// 测试用：库级运行时是否已创建且尚未释放。
    pub(super) fn running() -> bool {
        RUNTIME
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_some()
    }
}

/// 登记插件库的执行环境；关闭 `tokio` feature 时为空操作。由 `export_plugin!` 的入口调用。
pub fn install() {
    #[cfg(feature = "tokio")]
    runtime::install();
}

/// 库级 tokio 运行时是否正在运行。供测试与诊断使用。
pub fn running() -> bool {
    #[cfg(feature = "tokio")]
    return runtime::running();
    #[cfg(not(feature = "tokio"))]
    false
}
