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

pub type RawConfig = Arc<dyn Any + Send + Sync>;

#[derive(Clone, Default)]
pub struct Inject {
    pub(crate) entries: Vec<Dependency>,
}

#[derive(Clone)]
pub(crate) struct Dependency {
    pub key: TypeId,
    pub name: &'static str,
    pub meta: RawConfig,
}

impl Inject {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with<K: ServiceKey>(self) -> Self {
        self.with_meta::<K>(K::Meta::default())
    }
    pub fn with_meta<K: ServiceKey>(mut self, meta: K::Meta) -> Self {
        let entry = Dependency {
            key: TypeId::of::<K>(),
            name: K::NAME,
            meta: Arc::new(meta),
        };
        if let Some(old) = self.entries.iter_mut().find(|old| old.key == entry.key) {
            *old = entry;
        } else {
            self.entries.push(entry);
        }
        self
    }
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|entry| entry.name).collect()
    }
    pub fn metadata<K: ServiceKey>(&self) -> Option<&K::Meta> {
        self.entries
            .iter()
            .find(|entry| entry.key == TypeId::of::<K>())
            .and_then(|entry| entry.meta.downcast_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    pub message: String,
    pub path: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
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
pub trait Config: Clone + Send + Sync + Sized + 'static {
    type Raw: Clone + Send + Sync + 'static;
    fn resolve(raw: &Self::Raw) -> std::result::Result<Self, ValidationError>;
}

impl Config for () {
    type Raw = ();
    fn resolve(_: &()) -> std::result::Result<Self, ValidationError> {
        Ok(())
    }
}

/// 组件声明依赖，并在统一上下文中登记效应。
pub trait Component: Send + Sync + 'static {
    const NAME: &'static str;
    type Config: Config;
    fn inject() -> Inject {
        Inject::new()
    }
    fn apply(ctx: Context, config: Self::Config) -> impl Future<Output = Result> + Send;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComponentId {
    Type(TypeId),
    Closure(u64),
}

type Resolve = dyn Fn(RawConfig) -> Result<RawConfig> + Send + Sync;
type Apply = dyn Fn(Context, RawConfig) -> Task<Result> + Send + Sync;

#[derive(Clone)]
pub struct ComponentDef {
    pub(crate) id: ComponentId,
    pub(crate) name: &'static str,
    pub(crate) inject: Inject,
    pub(crate) resolve: Arc<Resolve>,
    pub(crate) apply: Arc<Apply>,
}

impl ComponentDef {
    pub fn of<C: Component>() -> Self {
        Self {
            id: ComponentId::Type(TypeId::of::<C>()),
            name: C::NAME,
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

    pub fn new<R, F, Fut>(name: &'static str, inject: Inject, apply: F) -> Self
    where
        R: Clone + Send + Sync + 'static,
        F: Fn(Context, R) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result> + Send + 'static,
    {
        Self::validated(name, inject, |raw: &R| Ok(raw.clone()), apply)
    }

    pub fn validated<R, C, V, F, Fut>(
        name: &'static str,
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
            name,
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

    pub fn id(&self) -> ComponentId {
        self.id
    }
    pub fn name(&self) -> &'static str {
        self.name
    }
}

pub(crate) struct RuntimeData {
    pub component: ComponentDef,
    pub fibers: Vec<FiberId>,
}

pub struct InternalReplace;
impl crate::Event for InternalReplace {
    const NAME: &'static str = "internal/replace";
    type Args = Fiber;
    type Output = ();
    type Kind = crate::SyncKind;
}

fn forward(state: &State, component: &ComponentDef) -> ComponentDef {
    let mut target = component.clone();
    while let Some(next) = state.aliases.get(&target.id) {
        target = next.clone();
    }
    target
}

#[derive(Clone)]
pub struct Registry {
    ctx: Context,
}

impl Registry {
    pub fn len(&self) -> usize {
        self.ctx.kernel.state.lock().runtimes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
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
    pub fn contains(&self, component: &ComponentDef) -> bool {
        let state = self.ctx.kernel.state.lock();
        state.runtimes.contains_key(&forward(&state, component).id)
    }
    pub fn get(&self, component: &ComponentDef) -> Option<ComponentDef> {
        let state = self.ctx.kernel.state.lock();
        state
            .runtimes
            .get(&forward(&state, component).id)
            .map(|runtime| runtime.component.clone())
    }
    pub fn fibers(&self, component: &ComponentDef) -> Vec<Fiber> {
        let state = self.ctx.kernel.state.lock();
        state
            .runtimes
            .get(&forward(&state, component).id)
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
    pub fn replace(&self, old: &ComponentDef, next: &ComponentDef) -> Vec<Fiber> {
        let (fibers, old_runtime, old_alias, old_target_alias, previous) = {
            let mut state = self.ctx.kernel.state.lock();
            let source = forward(&state, old);
            let old_target_alias = state.aliases.remove(&next.id);
            let fibers = state
                .runtimes
                .get(&source.id)
                .map(|runtime| {
                    runtime
                        .fibers
                        .iter()
                        .map(|id| Fiber::handle(&self.ctx.kernel, *id, &state))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let (old_runtime, old_alias) = if source.id != next.id {
                (
                    state.runtimes.remove(&source.id),
                    state.aliases.insert(source.id, next.clone()),
                )
            } else {
                (None, None)
            };
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
                data.name = next.name;
                let component = data.component.replace(next.clone());
                let inject = std::mem::replace(&mut data.inject, next.inject.clone());
                previous.push((component, inject));
            }
            (fibers, old_runtime, old_alias, old_target_alias, previous)
        };
        drop((old_runtime, old_alias, old_target_alias, previous));
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
    pub fn registry(&self) -> Registry {
        Registry { ctx: self.clone() }
    }

    /// S-REG-2：按组件类型挂载实例。
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
            let component = forward(&state, component);
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
