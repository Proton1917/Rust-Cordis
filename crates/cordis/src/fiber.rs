//! 纤程状态、纪元与串行调和。

use crate::context::Scope;
use crate::effect::Effect;
use crate::kernel::{EffectId, FiberId, Kernel, State};
use crate::list::DisposableList;
use crate::registry::{self, ComponentDef, ComponentId, Inject, RawConfig};
use crate::{Context, EffectMeta, Error, Event, Result, SyncKind, Task, WaterfallKind};
use futures_util::{
    FutureExt,
    future::{join_all, poll_fn},
};
use std::{
    fmt,
    panic::{AssertUnwindSafe, Location, catch_unwind},
    sync::Arc,
    task::{Poll, Waker},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiberState {
    Pending,
    Loading,
    Active,
    Failed,
    Disposed,
    Unloading,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Epoch {
    Inactive,
    Active { revision: u64, impls: Vec<u64> },
}

pub(crate) struct FiberData {
    pub state: FiberState,
    pub alive: bool,
    pub effects: DisposableList<EffectId>,
    pub parent_scope: Option<Arc<Scope>>,
    pub scope: Option<Arc<Scope>>,
    pub component: Option<ComponentDef>,
    pub config: RawConfig,
    pub inject: Inject,
    pub revision: u64,
    pub target: Epoch,
    pub running_epoch: Epoch,
    pub inertia: bool,
    pub error: Option<Error>,
    pub waiters: Vec<Waker>,
    pub lifetime: Option<EffectId>,
    pub location: &'static Location<'static>,
}

impl FiberData {
    #[track_caller]
    pub fn root() -> Self {
        Self {
            state: FiberState::Active,
            alive: true,
            effects: DisposableList::default(),
            parent_scope: None,
            scope: None,
            component: None,
            config: Arc::new(()),
            inject: Inject::new(),
            revision: 0,
            target: Epoch::Active {
                revision: 0,
                impls: Vec::new(),
            },
            running_epoch: Epoch::Active {
                revision: 0,
                impls: Vec::new(),
            },
            inertia: false,
            error: None,
            waiters: Vec::new(),
            lifetime: None,
            location: Location::caller(),
        }
    }

    pub fn child(
        parent: Arc<Scope>,
        component: ComponentDef,
        config: RawConfig,
        location: &'static Location<'static>,
    ) -> Self {
        Self {
            state: FiberState::Pending,
            parent_scope: Some(parent),
            inject: component.inject.clone(),
            component: Some(component),
            config,
            target: Epoch::Inactive,
            running_epoch: Epoch::Inactive,
            location,
            ..Self::root()
        }
    }

    fn idle_state(&self) -> FiberState {
        if !self.alive {
            FiberState::Disposed
        } else if self.error.is_some() {
            FiberState::Failed
        } else if self.target != Epoch::Inactive {
            FiberState::Active
        } else {
            FiberState::Pending
        }
    }
}

#[derive(Clone)]
pub struct Fiber {
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) id: FiberId,
    pub(crate) scope: Arc<Scope>,
}

impl PartialEq for Fiber {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.kernel, &other.kernel)
    }
}
impl Eq for Fiber {}
impl fmt::Debug for Fiber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fiber")
            .field("name", &self.name())
            .field("state", &self.state())
            .finish()
    }
}

pub struct InternalStatus;
impl Event for InternalStatus {
    const NAME: &'static str = "internal/status";
    type Args = (Fiber, FiberState);
    type Output = ();
    type Kind = SyncKind;
}
pub struct InternalPlugin;
impl Event for InternalPlugin {
    const NAME: &'static str = "internal/plugin";
    type Args = Fiber;
    type Output = ();
    type Kind = SyncKind;
}
pub struct InternalCommit;
impl Event for InternalCommit {
    const NAME: &'static str = "internal/commit";
    type Args = (Fiber, RawConfig);
    type Output = ();
    type Kind = SyncKind;
}
pub struct InternalConfig;
impl Event for InternalConfig {
    const NAME: &'static str = "internal/config";
    type Args = (Fiber, RawConfig);
    type Output = RawConfig;
    type Kind = WaterfallKind;
}
pub struct InternalUpdate;
impl Event for InternalUpdate {
    const NAME: &'static str = "internal/update";
    type Args = RawConfig;
    type Output = Task<Result>;
    type Kind = WaterfallKind;
}

impl Fiber {
    pub(crate) fn handle(kernel: &Arc<Kernel>, id: FiberId, state: &State) -> Self {
        Self {
            kernel: kernel.clone(),
            id,
            scope: state.fibers[id]
                .scope
                .as_ref()
                .expect("fiber context installed")
                .clone(),
        }
    }
    pub fn name(&self) -> String {
        name_locked(&self.kernel.state.lock(), self.id)
    }
    pub fn state(&self) -> FiberState {
        self.kernel.state.lock().fibers[self.id].state
    }
    pub fn epoch(&self) -> Epoch {
        self.kernel.state.lock().fibers[self.id].target.clone()
    }
    pub fn is_alive(&self) -> bool {
        self.kernel.state.lock().fibers[self.id].alive
    }
    pub fn is_settled(&self) -> bool {
        !self.kernel.state.lock().fibers[self.id].inertia
    }
    pub fn context(&self) -> Context {
        Context {
            kernel: self.kernel.clone(),
            scope: self.scope.clone(),
        }
    }
    pub fn component_id(&self) -> Option<ComponentId> {
        self.kernel.state.lock().fibers[self.id]
            .component
            .as_ref()
            .map(|component| component.id)
    }
    pub fn config<R: Send + Sync + 'static>(&self) -> Option<Arc<R>> {
        self.kernel.state.lock().fibers[self.id]
            .config
            .clone()
            .downcast()
            .ok()
    }
    pub fn inject(&self) -> Inject {
        self.kernel.state.lock().fibers[self.id].inject.clone()
    }
    pub fn effects(&self) -> Vec<EffectMeta> {
        let state = self.kernel.state.lock();
        state.fibers[self.id]
            .effects
            .iter()
            .map(|id| state.effects[*id].meta(&state))
            .collect()
    }

    /// S-FBR-14、S-FBR-21：等待当前及随后串联的迁移完成。
    pub fn settled(&self) -> Task<Result> {
        self.wait(true)
    }
    pub(crate) fn wait(&self, report_error: bool) -> Task<Result> {
        let fiber = self.clone();
        Box::pin(poll_fn(move |cx| {
            let mut state = fiber.kernel.state.lock();
            let data = &mut state.fibers[fiber.id];
            if !data.inertia {
                return Poll::Ready(if report_error {
                    data.error.clone().map_or(Ok(()), Err)
                } else {
                    Ok(())
                });
            }
            if !data.waiters.iter().any(|waker| waker.will_wake(cx.waker())) {
                data.waiters.push(cx.waker().clone());
            }
            Poll::Pending
        }))
    }

    /// S-FBR-18：修订号使配置相同的重启具有新的纪元。
    pub fn restart(&self) -> Task<Result> {
        {
            let mut state = self.kernel.state.lock();
            let data = &mut state.fibers[self.id];
            if !data.alive {
                return Box::pin(async { Err(Error::inactive()) });
            }
            data.revision += 1;
        }
        refresh(&self.kernel, self.id);
        self.settled()
    }

    /// S-FBR-19、S-FBR-20：保留原始配置，将解析结果交给更新监听器。
    pub fn update<R: Clone + Send + Sync + 'static>(&self, config: R) -> Task<Result> {
        self.update_raw(Arc::new(config))
    }
    fn update_raw(&self, config: RawConfig) -> Task<Result> {
        let loaded = {
            let state = self.kernel.state.lock();
            let data = &state.fibers[self.id];
            if !data.alive {
                return Box::pin(async { Err(Error::inactive()) });
            }
            data.target != Epoch::Inactive
        };
        let resolved = if loaded {
            match self.resolve(config.clone()) {
                Ok(value) => Some(value),
                Err(error) => return Box::pin(async move { Err(error) }),
            }
        } else {
            None
        };
        let ctx = self.context();
        ctx.emit::<InternalCommit>(&(self.clone(), config.clone()));
        let previous = {
            let mut state = self.kernel.state.lock();
            state.fibers[self.id].error = None;
            std::mem::replace(&mut state.fibers[self.id].config, config)
        };
        drop(previous);
        let Some(resolved) = resolved else {
            return self.restart();
        };
        let fiber = self.clone();
        match ctx.waterfall_filtered::<InternalUpdate>(
            resolved,
            move || Ok(fiber.restart()),
            Some(&|listener| listener.scope.fiber == self.id),
        ) {
            Ok(task) => task,
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }

    fn resolve(&self, raw: RawConfig) -> Result<RawConfig> {
        let component = self.kernel.state.lock().fibers[self.id].component.clone();
        let ctx = self.context();
        catch_unwind(AssertUnwindSafe(|| {
            ctx.waterfall::<InternalConfig>((self.clone(), raw.clone()), move || match &component {
                Some(component) => (component.resolve)(raw.clone()),
                None => Ok(raw.clone()),
            })
        }))
        .unwrap_or_else(|payload| Err(Error::panic(payload)))
    }

    /// S-FBR-16、S-FBR-17：释放实例；根纤程通过重启回收全部效应。
    pub fn dispose(&self) -> Task<Result> {
        let lifetime = {
            let mut state = self.kernel.state.lock();
            let data = &state.fibers[self.id];
            if data.parent_scope.is_none() {
                drop(state);
                return self.restart();
            }
            data.lifetime
                .map(|id| Effect::handle(&self.kernel, id, &mut state))
        };
        if let Some(effect) = lifetime {
            effect.dispose()
        } else {
            self.wait(false)
        }
    }

    pub(crate) fn begin_dispose(&self) {
        let runtime = {
            let mut state = self.kernel.state.lock();
            state.fibers[self.id].alive = false;
            state.fibers[self.id].lifetime = None;
            registry::unregister(&mut state, self.id)
        };
        drop(runtime);
        self.emit_plugin();
        refresh(&self.kernel, self.id);
    }

    pub(crate) fn emit_plugin(&self) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
            self.context().emit::<InternalPlugin>(self)
        })) {
            self.kernel
                .state
                .lock()
                .errors
                .push((self.id, Error::panic(payload)));
        }
    }
}

impl Context {
    /// T-LIF-7：组件在异步步骤边界检查其加载纪元。
    pub fn is_current(&self) -> bool {
        let state = self.kernel.state.lock();
        let data = &state.fibers[self.scope.fiber];
        data.alive && data.target == data.running_epoch
    }
}

/// T-LIF-3、A-5：目标变化入队，由唯一调和者同步决定迁移。
pub(crate) fn refresh(kernel: &Arc<Kernel>, id: FiberId) {
    {
        let mut state = kernel.state.lock();
        state.dirty.push_back(id);
        if state.draining {
            return;
        }
        state.draining = true;
    }
    loop {
        let action = {
            let mut state = kernel.state.lock();
            let Some(id) = state.dirty.pop_front() else {
                state.draining = false;
                return;
            };
            let Some(data) = state.fibers.get_mut(id) else {
                continue;
            };
            if !data.alive && data.scope.is_none() {
                continue;
            }
            let target = if data.alive {
                Epoch::Active {
                    revision: data.revision,
                    impls: Vec::new(),
                }
            } else {
                Epoch::Inactive
            };
            if data.error.is_some() && data.alive {
                continue;
            }
            let old_target = std::mem::replace(&mut data.target, target);
            if data.inertia {
                continue;
            }
            let old_state = data.state;
            if data.target == old_target {
                data.state = data.idle_state();
                (id, old_state, data.state, None)
            } else {
                let loading = old_target == Epoch::Inactive && data.target != Epoch::Inactive;
                data.inertia = true;
                data.state = if loading {
                    FiberState::Loading
                } else {
                    FiberState::Unloading
                };
                if loading {
                    data.running_epoch = data.target.clone();
                }
                (id, old_state, data.state, Some(loading))
            }
        };
        let (id, old, new, transition) = action;
        status_changed(kernel, id, old, new);
        if let Some(loading) = transition {
            let kernel = kernel.clone();
            let spawner = kernel.spawner.clone();
            spawner.spawn(Box::pin(async move {
                transition_loop(kernel, id, loading).await;
            }));
        } else if new == FiberState::Disposed {
            let fiber = Fiber::handle(kernel, id, &kernel.state.lock());
            release_record(&fiber);
        }
    }
}

fn status_changed(kernel: &Arc<Kernel>, id: FiberId, old: FiberState, new: FiberState) {
    if old == new {
        return;
    }
    let fiber = Fiber::handle(kernel, id, &kernel.state.lock());
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
        fiber
            .context()
            .emit::<InternalStatus>(&(fiber.clone(), old))
    })) {
        kernel.state.lock().errors.push((id, Error::panic(payload)));
    }
}

/// T-LIF-4、T-LIF-5、T-LIF-6：迁移结束后串联到最新目标纪元。
async fn transition_loop(kernel: Arc<Kernel>, id: FiberId, mut loading: bool) {
    loop {
        let fiber = Fiber::handle(&kernel, id, &kernel.state.lock());
        if loading {
            let epoch = kernel.state.lock().fibers[id].running_epoch.clone();
            let mut yielded = false;
            poll_fn(|cx| {
                if yielded {
                    Poll::Ready(())
                } else {
                    yielded = true;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            })
            .await;
            if kernel.state.lock().fibers[id].target == epoch {
                let (component, config, location) = {
                    let state = kernel.state.lock();
                    let data = &state.fibers[id];
                    (data.component.clone(), data.config.clone(), data.location)
                };
                let result = if let Some(component) = component {
                    let call = catch_unwind(AssertUnwindSafe(|| {
                        let config = fiber.resolve(config)?;
                        Ok((component.apply)(fiber.context(), config))
                    }));
                    match call {
                        Ok(Ok(task)) => AssertUnwindSafe(task)
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|payload| Err(Error::panic(payload))),
                        Ok(Err(error)) => Err(error),
                        Err(payload) => Err(Error::panic(payload)),
                    }
                } else {
                    Ok(())
                };
                if let Err(error) = result {
                    let error = error.at(location);
                    let mut state = kernel.state.lock();
                    state.errors.push((id, error.clone()));
                    let data = &mut state.fibers[id];
                    if data.target == epoch {
                        data.error = Some(error);
                        data.target = Epoch::Inactive;
                    }
                }
            }
        } else {
            let effects = {
                let mut state = kernel.state.lock();
                let ids = state.fibers[id].effects.take();
                ids.into_iter()
                    .rev()
                    .map(|id| Effect::handle(&kernel, id, &mut state))
                    .collect::<Vec<_>>()
            };
            let tasks = effects.iter().map(Effect::dispose).collect::<Vec<_>>();
            for result in join_all(tasks).await {
                if let Err(error) = result {
                    kernel.state.lock().errors.push((id, error));
                }
            }
        }
        let (old, new, next, waiters) = {
            let mut state = kernel.state.lock();
            let data = &mut state.fibers[id];
            let next = if loading {
                (data.target != data.running_epoch).then_some(false)
            } else {
                (data.target != Epoch::Inactive).then_some(true)
            };
            let old = data.state;
            let waiters = if let Some(next_loading) = next {
                data.state = if next_loading {
                    FiberState::Loading
                } else {
                    FiberState::Unloading
                };
                if next_loading {
                    data.running_epoch = data.target.clone();
                }
                Vec::new()
            } else {
                data.inertia = false;
                data.state = data.idle_state();
                std::mem::take(&mut data.waiters)
            };
            (old, data.state, next, waiters)
        };
        status_changed(&kernel, id, old, new);
        for waiter in waiters {
            waiter.wake();
        }
        let Some(next) = next else {
            if new == FiberState::Disposed {
                release_record(&fiber);
            }
            return;
        };
        loading = next;
    }
}

pub(crate) fn name_locked(state: &State, mut id: FiberId) -> String {
    loop {
        let data = &state.fibers[id];
        if let Some(component) = &data.component
            && !component.name.is_empty()
        {
            return component.name.into();
        }
        let Some(parent) = &data.parent_scope else {
            return "root".into();
        };
        id = parent.fiber;
    }
}

fn release_record(fiber: &Fiber) {
    let removed = {
        let mut state = fiber.kernel.state.lock();
        let data = &mut state.fibers[fiber.id];
        (
            data.scope.take(),
            data.component.take(),
            std::mem::replace(&mut data.config, Arc::new(())),
        )
    };
    drop(removed);
}
