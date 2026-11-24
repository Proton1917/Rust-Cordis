//! 纤程生命周期的纯决策边界。
//!
//! 状态、纪元和资源仍由 Kernel 持有。本模块只从该所有者给出的事实决定
//! 下一步；分支由 `formal/Lifecycle.lean` 的已证明决策树生成。

// Keep generated branches in one-to-one correspondence with proof-tree nodes.
#![allow(clippy::if_same_then_else, clippy::needless_bool)]

/// S-FBR-1：组件实例最近一次生命周期迁移记录的状态。
///
/// 实例的存活性由 [`crate::Fiber::is_alive`] 查询，调和完成性由
/// [`crate::Fiber::is_settled`] 查询。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiberState {
    /// 实例存活，等待声明的依赖全部满足。
    Pending,
    /// 正在执行一次加载迁移。
    Loading,
    /// 本次加载已经完成，加载纪元与目标纪元一致。
    Active,
    /// 加载或宿主派发失败；[`crate::Fiber::settled`] 返回相应错误。
    Failed,
    /// 实例释放迁移已经完成，生命周期登记已撤回。
    Disposed,
    /// 正在等待依赖方清理，并恢复本次加载登记的效应。
    Unloading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Keep,
    Idle,
    Load,
    Unload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TargetDecision {
    Keep,
    Resolved,
    Inactive,
}

include!("lifecycle_generated.rs");
