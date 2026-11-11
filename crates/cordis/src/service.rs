//! 类型化服务键、调用方句柄与元数据。

use crate::{Context, Effect, Logger, Result, Task, kernel::ImplId};
use std::{fmt, marker::PhantomData, ops::Deref, sync::Arc};

/// T-COE-9：定义服务键对应的元数据合并运算。
///
/// 实现以 `Default::default()` 为单位元，并满足结合律。
/// `next` 表示后加入的层，冲突字段取后层值；集合等字段可定义相应的合并规则。
/// 内核按声明、外层上下文、内层上下文的顺序调用该运算。
pub trait Merge: Clone + Default + Send + Sync + 'static {
    /// 将前层 `self` 与后层 `next` 合并，返回该键的合并元数据。
    fn merge(&self, next: &Self) -> Self;
}
impl Merge for () {
    fn merge(&self, _: &Self) -> Self {}
}

/// T-COE-1：以独立键类型关联服务值、访问元数据和可读名称。
///
/// 内核以键类型的 `TypeId` 区分服务，以隔离域区分同一键的绑定。
/// 每个键的值通过 `Arc<Self::Value>` 提供，读取时返回相同值类型的 `Scoped<Self>`。
pub trait ServiceKey: 'static {
    /// 服务名称，供错误消息、效应标签和服务通知使用。
    ///
    /// 同一内核中的不同键类型登记相同名称时，后一次提供返回错误。
    const NAME: &'static str;
    /// 该键对应的服务值类型，可以采用 `dyn Trait` 等动态大小类型。
    type Value: ?Sized + Send + Sync + 'static;
    /// 依赖声明和上下文拦截携带的元数据类型；空元数据使用 `()`。
    type Meta: Merge;
}

/// T-COE-1、T-CTX-1：保存一次服务读取所得的值及其访问上下文。
///
/// `Deref` 提供对 `K::Value` 的共享访问；克隆句柄共享该值的 `Arc`。
/// `caller` 承载使用点效应，`definition` 解析服务自身依赖，
/// `requester` 定位当前访问代码的依赖声明。
pub struct Scoped<K: ServiceKey> {
    pub(crate) value: Arc<K::Value>,
    pub(crate) caller: Context,
    pub(crate) definition: Context,
    pub(crate) requester: Context,
}

impl<K: ServiceKey> Clone for Scoped<K> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            caller: self.caller.clone(),
            definition: self.definition.clone(),
            requester: self.requester.clone(),
        }
    }
}
impl<K: ServiceKey> Deref for Scoped<K> {
    type Target = K::Value;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}
impl<K: ServiceKey> Scoped<K> {
    /// 返回原使用点上下文，服务方法在此登记效应及读取使用点的隔离、拦截视图。
    ///
    /// 经由 `dependency` 进行嵌套服务访问时，该上下文继续沿用原使用点。
    pub fn caller(&self) -> &Context {
        &self.caller
    }
    /// 返回提供当前绑定的上下文，服务自身的依赖通过此定义点解析。
    pub fn definition(&self) -> &Context {
        &self.definition
    }
    /// 返回发起当前访问的代码定义点，供声明元数据和关联服务访问使用。
    ///
    /// 直接读取时等于调用上下文；经由 `dependency` 读取时等于外层服务的定义点。
    pub fn requester(&self) -> &Context {
        &self.requester
    }
    /// 克隆并返回本句柄保存的值 `Arc`，延长该值的共享持有时间。
    ///
    /// 后续 `Provision::set` 替换绑定值时，已有句柄继续持有本次取得的值。
    pub fn value(&self) -> Arc<K::Value> {
        self.value.clone()
    }

    /// T-COE-10、S-SVC-11、S-SVC-12：返回此次访问的合并元数据。
    ///
    /// 从 `requester` 定位最近依赖声明，再依次合并 `caller` 的外层、内层拦截。
    /// 自供绑定或隔离边界截断的声明以 `K::Meta::default()` 作为起始值。
    /// 用户定义的克隆与合并操作在内核锁外执行。
    pub fn meta(&self) -> K::Meta {
        self.caller
            .merge_meta::<K>(crate::reflect::injected_meta::<K>(&self.requester))
    }

    /// 从当前服务的 `definition` 读取键 `D`，返回携带原使用点的依赖句柄。
    ///
    /// 读取采用 `Context::get` 的授权和已提交视图规则，并派发 `InternalGet`。
    /// 返回句柄的 `requester` 为当前服务定义点，`caller` 保留原使用点；
    /// 访问失败时返回 `Context::get` 产生的错误。
    #[track_caller]
    pub fn dependency<D: ServiceKey>(&self) -> Result<Scoped<D>> {
        let mut dependency = self.definition.get::<D>()?;
        dependency.caller = self.caller.clone();
        Ok(dependency)
    }

    /// 用 `value` 构造派生句柄，保留 `caller`、`definition` 和 `requester`。
    ///
    /// 新句柄直接持有传入的 `Arc`，内核中的服务绑定继续使用原值。
    pub fn with_value(&self, value: Arc<K::Value>) -> Self {
        Self {
            value,
            ..self.clone()
        }
    }

    /// 返回服务日志器，来源取 `definition`，名称和级别接受 `caller` 的日志拦截。
    ///
    /// 通过该日志器登记的导出器效应归属于原使用点 `caller`。
    pub fn logger(&self) -> Logger {
        Logger::for_service(self.definition.clone(), &self.caller)
    }
}
impl<K: ServiceKey> fmt::Debug for Scoped<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scoped")
            .field("service", &K::NAME)
            .finish_non_exhaustive()
    }
}

/// S-SVC-10：持有一个服务绑定的修改和撤回能力。
///
/// 克隆句柄共享同一绑定及其效应。释放由 `dispose` 或所属纤程卸载触发；
/// 已提交消费者清理期间，撤回的绑定记录继续保留，供该次加载完成清理。
pub struct Provision<K: ServiceKey> {
    pub(crate) effect: Effect,
    pub(crate) binding: ImplId,
    pub(crate) marker: PhantomData<fn() -> K>,
}
impl<K: ServiceKey> Clone for Provision<K> {
    fn clone(&self) -> Self {
        Self {
            effect: self.effect.clone(),
            binding: self.binding,
            marker: PhantomData,
        }
    }
}
impl<K: ServiceKey> Provision<K> {
    /// 返回登记服务的效应句柄，供 `EffectScope::adopt` 组合释放顺序。
    pub fn effect(&self) -> Effect {
        self.effect.clone()
    }
    /// S-SVC-2：立即开始撤回绑定，返回等待消费者清理和绑定记录释放的任务。
    ///
    /// 撤回会通知同域消费者重新解析依赖；重复调用共享同次释放结果。
    pub fn dispose(&self) -> Task<Result> {
        self.effect.dispose()
    }
    /// 将当前绑定的值替换为 `value`，成功返回 `()`。
    ///
    /// 绑定身份与消费者纪元保持原值，后续读取取得新的 `Arc`。
    /// 已撤回或已释放的绑定返回错误；修改过程派发 `InternalSet` 瀑布。
    /// 可用性条件变化后，通过 `notify` 重新评估消费者依赖。
    #[track_caller]
    pub fn set(&self, value: Arc<K::Value>) -> Result {
        crate::reflect::set::<K>(&self.effect.kernel, self.binding, value)
    }
    /// S-SVC-5：按本绑定的键和隔离域重新检查当前服务及消费者依赖。
    ///
    /// 检查在锁外调用可用性谓词，随后推进纤程调和并派发 `InternalService`。
    /// 绑定记录已经回收时，调用直接结束。
    pub fn notify(&self) {
        crate::reflect::notify_binding(&self.effect.kernel, self.binding);
    }
}
