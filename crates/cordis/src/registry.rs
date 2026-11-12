//! 组件定义、配置解析与运行时登记。

use crate::context::{Layer, Scope};
use crate::fiber::FiberData;
use crate::kernel::{FiberId, State};
use crate::{Context, Error, Fiber, Result, ServiceKey, Task};
use std::{
    any::{Any, TypeId},
    fmt,
    future::Future,
    panic::Location,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// 原始配置与配置事件解析结果的共享容器。
///
/// 容器保留值的具体 Rust 类型，可通过 `Any::downcast_ref` 或 `Arc::downcast` 读取。
pub type RawConfig = Arc<dyn Any + Send + Sync>;

/// ADR-0016：声明方对实际绑定值执行的锁外可用性判断。
pub type DependencyPredicate = Arc<dyn Fn(&crate::AnyArc) -> bool + Send + Sync>;

/// T-COE-4：按声明顺序保存组件需要的服务键及各键的元数据。
///
/// 同一键重复加入时替换该键的元数据，并保留它原来的声明位置。
#[derive(Clone, Default)]
pub struct Inject {
    pub(crate) entries: Vec<Dependency>,
}

#[derive(Clone)]
pub(crate) struct Dependency {
    pub key: Arc<str>,
    pub meta: Option<RawConfig>,
    pub required: bool,
    pub predicate: Option<DependencyPredicate>,
}

impl Inject {
    /// 创建空的依赖规格。
    pub fn new() -> Self {
        Self::default()
    }
    /// 加入服务键 `K`，使用 `K::Meta::default()` 作为声明元数据。
    pub fn with<K: ServiceKey>(self) -> Self {
        self.with_meta::<K>(K::Meta::default())
    }
    /// 加入服务键 `K`，以 `meta` 作为组件访问该服务时的声明元数据。
    pub fn with_meta<K: ServiceKey>(self, meta: K::Meta) -> Self {
        self.named_entry(K::NAME.into(), Some(Arc::new(meta)))
    }
    /// 按声明顺序返回各服务键的名称。
    pub fn names(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.key.as_ref())
            .collect()
    }
    /// 读取键 `K` 的声明元数据；规格中缺少该键时返回 `None`。
    pub fn metadata<K: ServiceKey>(&self) -> Option<&K::Meta> {
        self.entries
            .iter()
            .find(|entry| entry.key.as_ref() == K::NAME)
            .and_then(|entry| entry.meta.as_ref()?.downcast_ref())
    }
    /// K-2：按运行时名称声明依赖，名称与类型化键共享解析空间。
    pub fn with_named(self, name: impl Into<Arc<str>>) -> Self {
        self.named_entry(name.into(), None)
    }
    /// T-COE-10、K-2：按名称声明依赖及其拥有型元数据。
    pub fn with_named_meta(self, name: impl Into<Arc<str>>, meta: crate::AnyArc) -> Self {
        self.named_entry(name.into(), Some(meta))
    }
    /// ADR-0016：按名称声明可选性与该使用方的可用性条件。
    pub fn with_named_options(
        self,
        name: impl Into<Arc<str>>,
        required: bool,
        predicate: Option<DependencyPredicate>,
    ) -> Self {
        self.insert_entry(Dependency {
            key: name.into(),
            meta: None,
            required,
            predicate,
        })
    }
    fn named_entry(self, key: Arc<str>, meta: Option<RawConfig>) -> Self {
        self.insert_entry(Dependency {
            key,
            meta,
            required: true,
            predicate: None,
        })
    }
    fn insert_entry(mut self, entry: Dependency) -> Self {
        if let Some(old) = self.entries.iter_mut().find(|old| old.key == entry.key) {
            *old = entry;
        } else {
            self.entries.push(entry);
        }
        self
    }
}

/// S-REG-6：配置校验发现的一项问题。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    /// 面向调用方的问题说明。
    pub message: String,
    /// 从配置根开始的字段路径；数组下标使用其字符串形式，空路径表示配置整体。
    pub path: Vec<String>,
}

/// S-REG-6：包含结构化问题列表的配置校验错误。
///
/// 转换为 [`Error`] 后保留问题列表；显示文本按每条问题一行列出消息及点分路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    /// 解析器发现的问题，按展示顺序保存。
    pub issues: Vec<ValidationIssue>,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid config:")?;
        for issue in &self.issues {
            write!(f, "\n  - {}", issue.message)?;
            if !issue.path.is_empty() {
                write!(f, " (at {})", issue.path.join("."))?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

/// S-REG-6：原始配置与组件收到的解析配置具有各自的类型。
///
/// 每次加载调用 [`Config::resolve`]，将保存的 [`Config::Raw`] 解析为组件使用的值。
/// 单元类型 `()` 可用于单元配置。
pub trait Config: Clone + Send + Sync + Sized + 'static {
    /// [`Context::mount`] 与 [`Fiber::update`] 接受并保存在实例中的原始输入类型。
    type Raw: Clone + Send + Sync + 'static;
    /// 同步解析原始输入；成功返回组件配置，失败返回问题及字段路径。
    fn resolve(raw: &Self::Raw) -> std::result::Result<Self, ValidationError>;
}

impl Config for () {
    type Raw = ();
    fn resolve(_: &()) -> std::result::Result<Self, ValidationError> {
        Ok(())
    }
}

/// T-LIF-1、T-LIF-9：声明依赖并在统一上下文中登记效应的类型组件。
///
/// 同一组件类型的多个实例共享运行时定义，各自拥有配置、依赖绑定及效应。
pub trait Component: Send + Sync + 'static {
    /// 组件名称，供纤程名称解析及日志使用；空字符串继承祖先名称。
    const NAME: &'static str;
    /// 组件的解析配置类型，其 [`Config::Raw`] 给出挂载时接受的输入类型。
    type Config: Config;
    /// 返回本组件的依赖规格，默认值为空规格。
    fn inject() -> Inject {
        Inject::new()
    }
    /// 在实例自身的 `ctx` 中执行加载，`config` 为解析后的配置。
    ///
    /// 返回的 Future 完成后进入下一生命周期状态。执行错误触发已登记效应的恢复；
    /// 组件可通过 [`Context::is_current`] 在异步步骤之间协作响应目标变化。
    fn apply(ctx: Context, config: Self::Config) -> impl Future<Output = Result> + Send;
}

/// S-REG-1：登记组件运行时所使用的身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComponentId {
    /// 类型组件的身份。
    Type(
        /// 组件类型的 `TypeId`。
        TypeId,
    ),
    /// 闭包定义创建时分配的身份。
    Closure(
        /// 当前定义的编号。
        u64,
    ),
}

type Resolve = dyn Fn(RawConfig) -> Result<RawConfig> + Send + Sync;
type Apply = dyn Fn(Context, RawConfig) -> Task<Result> + Send + Sync;

/// 可复用的组件定义，保存身份、名称、依赖规格、配置解析函数及加载函数。
///
/// 克隆保留组件身份；将同一定义多次传给 [`Context::mount_with`] 可创建独立实例。
#[derive(Clone)]
pub struct ComponentDef {
    pub(crate) id: ComponentId,
    pub(crate) name: Arc<str>,
    pub(crate) inject: Inject,
    pub(crate) resolve: Arc<Resolve>,
    pub(crate) apply: Arc<Apply>,
}

impl ComponentDef {
    /// 从组件类型 `C` 构造定义，使用 `TypeId::of::<C>()` 作为身份。
    pub fn of<C: Component>() -> Self {
        Self {
            id: ComponentId::Type(TypeId::of::<C>()),
            name: C::NAME.into(),
            inject: C::inject(),
            resolve: Arc::new(|raw| {
                let raw = raw
                    .downcast_ref::<<C::Config as Config>::Raw>()
                    .ok_or_else(|| Error::new("invalid config type"))?;
                C::Config::resolve(raw)
                    .map(|value| Arc::new(value) as RawConfig)
                    .map_err(Error::from)
            }),
            apply: Arc::new(|ctx, config| {
                let config = config
                    .downcast_ref::<C::Config>()
                    .expect("resolved configuration type")
                    .clone();
                Box::pin(C::apply(ctx, config))
            }),
        }
    }

    /// 创建新的闭包组件定义，`R` 同时作为原始配置类型和加载函数接收的配置类型。
    ///
    /// `name` 决定组件名称，`inject` 声明依赖，`apply` 接收实例上下文及配置副本。
    /// 每次构造分配一个新的组件身份。
    pub fn new<R, F, Fut>(name: impl Into<Arc<str>>, inject: Inject, apply: F) -> Self
    where
        R: Clone + Send + Sync + 'static,
        F: Fn(Context, R) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result> + Send + 'static,
    {
        Self::validated(name, inject, |raw: &R| Ok(raw.clone()), apply)
    }

    /// 创建带配置解析函数的闭包组件定义。
    ///
    /// `resolve` 将原始输入 `R` 同步解析为 `C`，`apply` 接收实例上下文及解析值。
    /// `name` 与 `inject` 给出名称和依赖规格；每次构造分配一个新的组件身份。
    /// 解析错误通过实例的等待任务返回，并保留 [`ValidationError`] 的问题列表。
    pub fn validated<R, C, V, F, Fut>(
        name: impl Into<Arc<str>>,
        inject: Inject,
        resolve: V,
        apply: F,
    ) -> Self
    where
        R: Clone + Send + Sync + 'static,
        C: Clone + Send + Sync + 'static,
        V: Fn(&R) -> std::result::Result<C, ValidationError> + Send + Sync + 'static,
        F: Fn(Context, C) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result> + Send + 'static,
    {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: ComponentId::Closure(NEXT.fetch_add(1, Ordering::Relaxed)),
            name: name.into(),
            inject,
            resolve: Arc::new(move |raw| {
                let raw = raw
                    .downcast_ref::<R>()
                    .ok_or_else(|| Error::new("invalid config type"))?;
                resolve(raw)
                    .map(|value| Arc::new(value) as RawConfig)
                    .map_err(Error::from)
            }),
            apply: Arc::new(move |ctx, raw| {
                let config = raw
                    .downcast_ref::<C>()
                    .expect("resolved configuration type")
                    .clone();
                Box::pin(apply(ctx, config))
            }),
        }
    }

    /// 返回该定义的组件身份。
    pub fn id(&self) -> ComponentId {
        self.id
    }
    /// 返回该定义保存的名称；空字符串表示实例继承祖先名称。
    pub fn name(&self) -> &str {
        &self.name
    }
}

pub(crate) struct RuntimeData {
    pub component: ComponentDef,
    pub fibers: Vec<FiberId>,
}

/// S-REG-4：实例改用新定义及规格后、重新检查依赖之前发送的同步通知。
///
/// 参数为已迁移的 [`Fiber`]，监听器的输出类型为 `()`。
pub struct InternalReplace;
impl crate::Event for InternalReplace {
    const NAME: &'static str = "internal/replace";
    type Args = Fiber;
    type Output = ();
    type Kind = crate::SyncKind;
}

fn forward(state: &State, mut target: ComponentId) -> ComponentId {
    while let Some(next) = state.aliases.get(&target) {
        target = *next;
    }
    target
}

/// 当前内核共享组件登记表的句柄。
///
/// 查询按组件身份定位运行时；[`Registry::contains`]、[`Registry::get`] 和
/// [`Registry::fibers`] 沿组件替换产生的别名链解析目标。
#[derive(Clone)]
pub struct Registry {
    ctx: Context,
}

impl Registry {
    /// ADR-0016、K-4：实例登记清空后显式退役转发终点与相关别名。
    pub fn forget(&self, component: &ComponentDef) -> Result<()> {
        let removed = {
            let mut state = self.ctx.kernel.state.lock();
            let target = forward(&state, component.id);
            if state.runtimes.contains_key(&target) {
                return Err(Error::new("component still has registered instances"));
            }
            let aliases: Vec<_> = state
                .aliases
                .keys()
                .copied()
                .filter(|id| forward(&state, *id) == target)
                .collect();
            for alias in aliases {
                state.aliases.remove(&alias);
            }
            state.definitions.remove(&target)
        };
        drop(removed);
        Ok(())
    }
    /// 返回保有登记实例的组件运行时数量。
    pub fn len(&self) -> usize {
        self.ctx.kernel.state.lock().runtimes.len()
    }
    /// 登记表中组件运行时数量为零时返回 `true`。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// 返回当前登记的实际组件身份列表。
    pub fn keys(&self) -> Vec<ComponentId> {
        self.ctx
            .kernel
            .state
            .lock()
            .runtimes
            .keys()
            .copied()
            .collect()
    }
    /// 查询 `component` 经别名解析后的运行时是否仍有登记实例。
    pub fn contains(&self, component: &ComponentDef) -> bool {
        let state = self.ctx.kernel.state.lock();
        state.runtimes.contains_key(&forward(&state, component.id))
    }
    /// 返回 `component` 经别名解析后已登记的定义；目标运行时缺席时返回 `None`。
    pub fn get(&self, component: &ComponentDef) -> Option<ComponentDef> {
        let state = self.ctx.kernel.state.lock();
        state
            .runtimes
            .get(&forward(&state, component.id))
            .map(|runtime| runtime.component.clone())
    }
    /// 返回目标运行时的实例句柄快照，`component` 按别名链解析。
    ///
    /// 目标运行时缺席时返回空列表。
    pub fn fibers(&self, component: &ComponentDef) -> Vec<Fiber> {
        let state = self.ctx.kernel.state.lock();
        state
            .runtimes
            .get(&forward(&state, component.id))
            .map(|runtime| {
                runtime
                    .fibers
                    .iter()
                    .map(|id| Fiber::handle(&self.ctx.kernel, *id, &state))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// S-REG-4、S-REG-5：替换实现并保留每个现有纤程的身份。
    ///
    /// `old` 沿别名链定位待迁移的运行时，`next` 成为新的具体实现。
    /// 调用时同步替换定义和依赖规格、检查绑定并提交调和，保留各实例的原始配置
    /// 与父级位置。返回受影响的实例句柄，等待各句柄的 [`Fiber::settled`] 可取得加载结果。
    /// 既有的 `next` 实例继续归属同一运行时；旧组件之后的挂载沿别名链转发到新定义。
    /// 将某个定义再次作为 `next` 会解除它自身的转发关系。
    /// 旧运行时缺席时返回空列表，同时登记新的转发关系。
    pub fn replace(&self, old: &ComponentDef, next: &ComponentDef) -> Vec<Fiber> {
        let (fibers, old_runtime, old_definitions, previous) = {
            let mut state = self.ctx.kernel.state.lock();
            let source = forward(&state, old.id);
            state.aliases.remove(&next.id);
            let fibers = state
                .runtimes
                .get(&source)
                .map(|runtime| {
                    runtime
                        .fibers
                        .iter()
                        .map(|id| Fiber::handle(&self.ctx.kernel, *id, &state))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let old_runtime = if source != next.id {
                state.aliases.insert(source, next.id);
                state.runtimes.remove(&source)
            } else {
                None
            };
            let mut old_definitions = Vec::new();
            if state.aliases.values().any(|id| *id == next.id) {
                old_definitions.extend(state.definitions.insert(next.id, next.clone()));
            }
            let retired: Vec<_> = state
                .definitions
                .keys()
                .copied()
                .filter(|id| {
                    state.aliases.contains_key(id)
                        || !state.aliases.values().any(|target| target == id)
                })
                .collect();
            for id in retired {
                old_definitions.extend(state.definitions.remove(&id));
            }
            if !fibers.is_empty() {
                state
                    .runtimes
                    .entry(next.id)
                    .or_insert_with(|| RuntimeData {
                        component: next.clone(),
                        fibers: Vec::new(),
                    });
            }
            let mut previous = Vec::new();
            for fiber in &fibers {
                let runtime = state
                    .runtimes
                    .get_mut(&next.id)
                    .expect("replacement runtime installed");
                if !runtime.fibers.contains(&fiber.id) {
                    runtime.fibers.push(fiber.id);
                }
                let data = &mut state.fibers[fiber.id];
                data.name = next.name.clone();
                let component = data.component.replace(next.clone());
                let inject = std::mem::replace(&mut data.inject, next.inject.clone());
                previous.push((component, inject));
            }
            (fibers, old_runtime, old_definitions, previous)
        };
        drop((old_runtime, old_definitions, previous));
        for fiber in &fibers {
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                fiber.context().emit::<InternalReplace>(fiber)
            })) {
                self.ctx.kernel.report(fiber.id, Error::panic(payload));
            }
            {
                let mut state = self.ctx.kernel.state.lock();
                let data = &mut state.fibers[fiber.id];
                data.bindings = vec![None; data.inject.entries.len()];
                data.error = None;
                data.revision += 1;
                crate::fiber::enqueue_locked(&mut state, fiber.id);
            }
            crate::reflect::refresh(&self.ctx.kernel, fiber.id);
        }
        fibers
    }
}

impl Context {
    /// 返回当前内核的共享组件登记表句柄。
    pub fn registry(&self) -> Registry {
        Registry { ctx: self.clone() }
    }

    /// S-REG-2：按组件类型挂载实例。
    ///
    /// `config` 的类型由 `C::Config::Raw` 确定。创建后立即返回实例句柄，配置解析与
    /// 加载函数由生命周期任务执行，加载结果通过 [`Fiber::settled`] 取得。
    /// 依赖欠缺时实例保持 `Pending`，失活父上下文返回错误。
    ///
    /// ```compile_fail
    /// fn invalid(ctx: &cordis::Context) {
    ///     let _ = ctx.mount::<()>(());
    /// }
    /// ```
    #[track_caller]
    pub fn mount<C: Component>(&self, config: <C::Config as Config>::Raw) -> Result<Fiber> {
        self.mount_with(&ComponentDef::of::<C>(), config)
    }

    /// S-REG-1、S-FBR-15、T-LIF-8：子组件登记为父组件的效应。
    ///
    /// `component` 沿别名链解析，`config` 的具体类型 `R` 应与目标定义的原始配置类型一致。
    /// 创建后立即返回句柄，配置及加载错误由 [`Fiber::settled`] 交回；失活父上下文
    /// 在挂载时返回错误。父组件卸载时级联释放该实例。
    #[track_caller]
    pub fn mount_with<R: Clone + Send + Sync + 'static>(
        &self,
        component: &ComponentDef,
        config: R,
    ) -> Result<Fiber> {
        let location = Location::caller();
        let id = {
            let mut state = self.kernel.state.lock();
            if !state.fibers[self.scope.fiber].alive {
                return Err(Error::inactive().at(location));
            }
            let target = forward(&state, component.id);
            let component = if target == component.id {
                component.clone()
            } else {
                state.definitions[&target].clone()
            };
            let id = state.fibers.insert(FiberData::child(
                self.scope.clone(),
                component.clone(),
                Arc::new(config),
                location,
            ));
            let scope = Arc::new(Scope {
                parent: Some(self.scope.clone()),
                fiber: id,
                layer: Layer::Fiber,
                lease: Some(Arc::downgrade(&self.kernel)),
            });
            state.fibers[id].canonical_scope = Arc::downgrade(&scope);
            state.fibers[id].scope = Some(scope);
            state
                .runtimes
                .entry(component.id)
                .or_insert_with(|| RuntimeData {
                    component: component.clone(),
                    fibers: Vec::new(),
                })
                .fibers
                .push(id);
            id
        };
        let fiber = Fiber::handle(&self.kernel, id, &self.kernel.state.lock());
        let child = fiber.clone();
        let effect = self.effect("ctx.use()", move |fx| {
            fx.defer_async(move || {
                child.begin_dispose();
                async move { child.wait_completion().await }
            })
        });
        let effect = match effect {
            Ok(effect) => effect,
            Err(error) => {
                fiber.begin_dispose();
                return Err(error);
            }
        };
        self.kernel.state.lock().fibers[id].lifetime = Some(effect.id);
        fiber.emit_plugin();
        crate::reflect::refresh(&self.kernel, id);
        Ok(fiber)
    }

    /// S-REG-3：按 `inject` 创建匿名子组件。
    ///
    /// `apply` 接收该实例自身的上下文，其返回 Future 完成一次加载。
    /// 返回句柄、依赖等待及父级释放行为与 [`Context::mount_with`] 一致。
    #[track_caller]
    pub fn inject<F, Fut>(&self, inject: Inject, apply: F) -> Result<Fiber>
    where
        F: Fn(Context) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result> + Send + 'static,
    {
        self.mount_with(
            &ComponentDef::new("", inject, move |ctx, ()| apply(ctx)),
            (),
        )
    }
}

pub(crate) fn unregister(state: &mut State, id: FiberId) -> Option<RuntimeData> {
    let component = state.fibers[id].component.as_ref()?;
    let key = component.id;
    if let Some(runtime) = state.runtimes.get_mut(&key) {
        runtime.fibers.retain(|current| *current != id);
        if runtime.fibers.is_empty() {
            return state.runtimes.remove(&key);
        }
    }
    None
}
