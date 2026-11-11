//! 类型化服务键与调用方句柄。

use crate::{Context, Effect, Result, Task, kernel::ImplId};
use std::{fmt, marker::PhantomData, ops::Deref, sync::Arc};

/// T-COE-9：按声明、外层、内层的顺序合并元数据。
pub trait Merge: Clone + Default + Send + Sync + 'static {
    fn merge(&self, next: &Self) -> Self;
}

impl Merge for () {
    fn merge(&self, _: &Self) -> Self {}
}

pub trait ServiceKey: 'static {
    const NAME: &'static str;
    type Value: ?Sized + Send + Sync + 'static;
    type Meta: Merge;
}

/// T-COE-1、T-CTX-1：类型化值携带使用点上下文。
pub struct Scoped<K: ServiceKey> {
    pub(crate) value: Arc<K::Value>,
    pub(crate) caller: Context,
}

impl<K: ServiceKey> Clone for Scoped<K> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            caller: self.caller.clone(),
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
    pub fn caller(&self) -> &Context {
        &self.caller
    }
    pub fn value(&self) -> Arc<K::Value> {
        self.value.clone()
    }
}

impl<K: ServiceKey> fmt::Debug for Scoped<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scoped")
            .field("service", &K::NAME)
            .finish_non_exhaustive()
    }
}

/// S-SVC-10：绑定的修改能力随提供句柄传递。
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
    pub fn effect(&self) -> Effect {
        self.effect.clone()
    }
    pub fn dispose(&self) -> Task<Result> {
        self.effect.dispose()
    }

    #[track_caller]
    pub fn set(&self, value: Arc<K::Value>) -> Result {
        crate::reflect::set::<K>(&self.effect.kernel, self.binding, value)
    }

    pub fn notify(&self) {
        crate::reflect::notify_binding(&self.effect.kernel, self.binding);
    }
}
