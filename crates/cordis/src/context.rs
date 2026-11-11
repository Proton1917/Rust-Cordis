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

/// 服务解析使用的隔离域身份；克隆保持身份，`new` 与 `Default` 分配新身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Realm(u64);

impl Realm {
    /// 上下文在服务键上继承的初始隔离域。
    pub const DEFAULT: Self = Self(0);
    /// 分配一个供独立服务绑定使用的隔离域。
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

/// 持有内核与上下文视图的一等句柄，效应归属于视图对应的纤程。
///
/// 应用退出时显式等待 `root.fiber().dispose()`。效应闭包捕获上下文时，
/// `Kernel → 效应闭包 → Context → Kernel` 构成强引用环；
/// 根纤程释放会执行并移出闭包，解除循环引用并完成资源清理。
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
    /// `host` 接管生命周期任务，并为整个内核提供时钟。
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
    /// 返回携带新值的派生视图，父视图继续保留原有解析结果。
    pub fn extend<T: Send + Sync + 'static>(&self, value: T) -> Self {
        self.derive(Layer::Extend(TypeId::of::<T>(), Arc::new(value)))
    }

    /// 从当前视图向祖先查找类型 `T`，返回最近的共享值或 `None`。
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
    /// `K` 在返回视图中解析到 `realm`，其余服务键继续继承原有域。
    pub fn isolate<K: ServiceKey>(&self, realm: Realm) -> Self {
        self.derive(Layer::Isolate(TypeId::of::<K>(), realm))
    }

    /// 返回当前视图对服务键 `K` 解析出的隔离域。
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
    /// 返回附加 `meta` 的派生视图，合并顺序由 `K::Meta::merge` 定义。
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

    /// 从默认元数据开始，按外层到内层的顺序合并当前视图的拦截层。
    pub fn intercept_meta<K: ServiceKey>(&self) -> K::Meta {
        self.merge_meta::<K>(K::Meta::default())
    }

    /// 返回持有当前视图效应的纤程句柄。
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
    /// 读取宿主时间基准下的当前时间。
    pub fn now(&self) -> Duration {
        self.kernel.clock.now()
    }

    /// 返回由宿主时钟完成的等待任务，`duration` 为等待时长。
    pub fn sleep(&self, duration: Duration) -> crate::Task {
        self.kernel.clock.sleep(duration)
    }

    /// 取得共享日志缓冲中当前保留的错误值快照。
    pub fn errors(&self) -> Vec<crate::Error> {
        crate::logger::errors(&self.kernel)
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Context <{}>", self.fiber().name())
    }
}
