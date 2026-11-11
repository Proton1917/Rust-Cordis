//! 上下文句柄与持久视图链。

use crate::{
    Clock, Fiber, Merge, ServiceKey, Spawn,
    kernel::{FiberId, Kernel},
};
use std::{
    any::{Any, TypeId},
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Realm(u64);

impl Realm {
    pub const DEFAULT: Self = Self(0);
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for Realm {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct Context {
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) scope: Arc<Scope>,
}

pub(crate) struct Scope {
    pub parent: Option<Arc<Scope>>,
    pub fiber: FiberId,
    pub layer: Layer,
    pub lease: Option<Weak<Kernel>>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(kernel) = self.lease.as_ref().and_then(Weak::upgrade) {
            let removed = {
                let mut state = kernel.state.lock();
                if state
                    .fibers
                    .get(self.fiber)
                    .is_some_and(|fiber| !fiber.alive && !fiber.inertia && !fiber.queued)
                {
                    state.fibers.remove(self.fiber)
                } else {
                    None
                }
            };
            drop(removed);
        }
    }
}

pub(crate) enum Layer {
    Root,
    Fiber,
    Extend(TypeId, Arc<dyn Any + Send + Sync>),
    Isolate(TypeId, Realm),
    Intercept(TypeId, Arc<dyn Any + Send + Sync>),
}

impl Context {
    /// S-CTX-1、T-CTX-1：创建根纤程及其统一上下文。
    pub fn new<H: Spawn + Clock>(host: Arc<H>) -> Self {
        let (kernel, fiber) = Kernel::new(host);
        let scope = Arc::new(Scope {
            parent: None,
            fiber,
            layer: Layer::Root,
            lease: None,
        });
        {
            let mut state = kernel.state.lock();
            state.fibers[fiber].canonical_scope = Arc::downgrade(&scope);
            state.fibers[fiber].scope = Some(scope.clone());
        }
        Self { kernel, scope }
    }

    pub(crate) fn derive(&self, layer: Layer) -> Self {
        Self {
            kernel: self.kernel.clone(),
            scope: Arc::new(Scope {
                parent: Some(self.scope.clone()),
                fiber: self.scope.fiber,
                layer,
                lease: None,
            }),
        }
    }

    /// S-CTX-2：类型相同的扩展值由最近一层覆盖。
    pub fn extend<T: Send + Sync + 'static>(&self, value: T) -> Self {
        self.derive(Layer::Extend(TypeId::of::<T>(), Arc::new(value)))
    }

    pub fn extension<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let mut scope = Some(&self.scope);
        while let Some(node) = scope {
            if let Layer::Extend(key, value) = &node.layer
                && *key == TypeId::of::<T>()
            {
                return value.clone().downcast().ok();
            }
            scope = node.parent.as_ref();
        }
        None
    }

    /// S-CTX-3、T-COE-7：派生键的隔离视图。
    pub fn isolate<K: ServiceKey>(&self, realm: Realm) -> Self {
        self.derive(Layer::Isolate(TypeId::of::<K>(), realm))
    }

    pub fn realm<K: ServiceKey>(&self) -> Realm {
        self.resolve_realm(TypeId::of::<K>())
    }

    pub(crate) fn resolve_realm(&self, key: TypeId) -> Realm {
        let mut scope = Some(&self.scope);
        while let Some(node) = scope {
            if let Layer::Isolate(k, realm) = node.layer
                && key == k
            {
                return realm;
            }
            scope = node.parent.as_ref();
        }
        Realm::DEFAULT
    }

    /// S-CTX-4、T-COE-11、T-COE-13：保留拦截层，访问时合并。
    pub fn intercept<K: ServiceKey>(&self, meta: K::Meta) -> Self {
        self.derive(Layer::Intercept(TypeId::of::<K>(), Arc::new(meta)))
    }

    pub(crate) fn merge_meta<K: ServiceKey>(&self, initial: K::Meta) -> K::Meta {
        let mut layers = Vec::new();
        let mut scope = Some(&self.scope);
        while let Some(node) = scope {
            if let Layer::Intercept(key, value) = &node.layer
                && *key == TypeId::of::<K>()
            {
                layers.push(
                    value
                        .downcast_ref::<K::Meta>()
                        .expect("metadata type follows key"),
                );
            }
            scope = node.parent.as_ref();
        }
        layers
            .into_iter()
            .rev()
            .fold(initial, |value, next| value.merge(next))
    }

    /// 返回上下文携带的拦截元数据。
    pub fn intercept_meta<K: ServiceKey>(&self) -> K::Meta {
        self.merge_meta::<K>(K::Meta::default())
    }

    pub fn fiber(&self) -> Fiber {
        let mut scope = &self.scope;
        while !matches!(scope.layer, Layer::Root | Layer::Fiber) {
            scope = scope.parent.as_ref().expect("derived context has a parent");
        }
        Fiber {
            kernel: self.kernel.clone(),
            id: self.scope.fiber,
            scope: scope.clone(),
        }
    }
    pub fn now(&self) -> Duration {
        self.kernel.clock.now()
    }

    pub fn sleep(&self, duration: Duration) -> crate::Task {
        self.kernel.clock.sleep(duration)
    }

    pub fn errors(&self) -> Vec<crate::Error> {
        crate::logger::errors(&self.kernel)
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Context <{}>", self.fiber().name())
    }
}
