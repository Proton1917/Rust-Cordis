//! ADR-0055：插件侧的执行环境钩子。
//!
//! 每个插件动态库静态链接自己的一份 cordis-abi，所以这里的静态变量只属于登记它的那个库。
//! SDK 在插件握手时登记钩子；宿主进程从不登记，边界函数在宿主一侧的行为保持不变。

use std::sync::OnceLock;

/// 插件库的执行环境：边界函数在执行用户代码前进入，描述符释放后关闭。
pub struct Ambient {
    /// 在插件的执行上下文中运行一次 `operation`。实现必须恰好调用一次。
    pub enter: fn(&mut dyn FnMut()),
    /// 插件库的描述符释放后调用一次，用于关闭库级资源。
    pub release: fn(),
}

static AMBIENT: OnceLock<Ambient> = OnceLock::new();

/// ADR-0055：登记本插件库的执行环境；同一库只接受第一次登记，返回是否登记成功。
pub fn install(ambient: Ambient) -> bool {
    AMBIENT.set(ambient).is_ok()
}

/// 在已登记的执行环境中运行 `operation`；未登记时直接运行。
pub(crate) fn within<R>(operation: impl FnOnce() -> R) -> R {
    let Some(ambient) = AMBIENT.get() else {
        return operation();
    };
    let mut operation = Some(operation);
    let mut result = None;
    (ambient.enter)(&mut || {
        if let Some(operation) = operation.take() {
            result = Some(operation());
        }
    });
    if let Some(result) = result {
        return result;
    }
    // 钩子没有调用 operation 时退回直接执行，保证用户代码恰好运行一次。
    let operation = operation.take().expect("operation 未运行时仍由调用方持有");
    operation()
}

/// 描述符释放后通知执行环境关闭库级资源。
pub(crate) fn release() {
    if let Some(ambient) = AMBIENT.get() {
        (ambient.release)();
    }
}
