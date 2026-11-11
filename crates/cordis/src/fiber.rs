//! 纤程状态、纪元与串行调和。

use crate::context::Scope;
use crate::effect::Effect;
use crate::kernel::{EffectId, FiberId, ImplId, Kernel, State};
use crate::list::DisposableList;
use crate::registry::{self, ComponentDef, ComponentId, Inject, RawConfig};
use crate::{Context, EffectMeta, Error, Event, Result, SyncKind, Task, WaterfallKind};
use futures_util::{
    FutureExt,
    future::{join_all, poll_fn},
};
use std::{
    any::TypeId,
    fmt,
    panic::{AssertUnwindSafe, Location, catch_unwind},
    sync::{Arc, Weak},
    task::{Poll, Waker},
};

/// S-FBR-1：组件实例最近一次生命周期迁移记录的状态。
///
/// 实例的存活性由 [`Fiber::is_alive`] 查询，调和完成性由 [`Fiber::is_settled`] 查询。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiberState {
    /// 实例存活，等待声明的依赖全部满足。
    Pending,
    /// 正在执行一次加载迁移。
    Loading,
    /// 本次加载已经完成，加载纪元与目标纪元一致。
    Active,
    /// 加载或宿主派发失败；[`Fiber::settled`] 返回相应错误。
    Failed,
    /// 实例释放迁移已经完成，生命周期登记已撤回。
    Disposed,
    /// 正在等待依赖方清理，并恢复本次加载登记的效应。
    Unloading,
}

/// T-LIF-2、S-FBR-5：用修订号和依赖绑定身份标识一次目标配置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Epoch {
    /// 非活跃目标，对应模型中的底元素 `⊥`。
    Inactive,
    /// 依赖满足时的目标配置；相等比较覆盖全部字段。
    Active {
        /// 显式重启、配置重载和组件替换推进的修订号。
        revision: u64,
        /// 已声明依赖的绑定编号，顺序与 [`Inject`] 的声明顺序一致。
        impls: Vec<u64>,
    },
}

pub(crate) struct FiberData {
    pub state: FiberState,
    pub alive: bool,
    pub effects: DisposableList<EffectId>,
    pub parent_scope: Option<Arc<Scope>>,
    pub scope: Option<Arc<Scope>>,
    pub canonical_scope: Weak<Scope>,
    pub component: Option<ComponentDef>,
    pub name: &'static str,
    pub config: RawConfig,
    pub inject: Inject,
    pub bindings: Vec<Option<ImplId>>,
    pub committed: Vec<(TypeId, ImplId)>,
    pub provisions: Vec<ImplId>,
    pub revision: u64,
    pub target: Epoch,
    pub running_epoch: Epoch,
    pub inertia: bool,
    pub queued: bool,
    pub notifications: usize,
    pub suspended: Option<Task>,
    pub dispatch_error: Option<Error>,
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
            canonical_scope: Weak::new(),
            component: None,
            name: "",
            config: Arc::new(()),
            inject: Inject::new(),
            bindings: Vec::new(),
            committed: Vec::new(),
            provisions: Vec::new(),
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
            queued: false,
            notifications: 0,
            suspended: None,
            dispatch_error: None,
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
            bindings: vec![None; component.inject.entries.len()],
            inject: component.inject.clone(),
            name: component.name,
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

/// T-LIF-8、T-LIF-9：组件实例的共享句柄。
///
/// 克隆的句柄控制同一实例。挂载操作登记为父级效应，资源回收由
/// [`Fiber::dispose`] 或父级卸载触发。句柄保留期间可以继续查询已释放实例的状态。
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

/// S-FBR-2：状态变化时发送的同步通知。
///
/// 参数为 `(Fiber, FiberState)`，第二项是变化前的状态；通过句柄读取当前状态。
/// 监听器的输出类型为 `()`。
pub struct InternalStatus;
impl Event for InternalStatus {
    const NAME: &'static str = "internal/status";
    type Args = (Fiber, FiberState);
    type Output = ();
    type Kind = SyncKind;
}
/// S-FBR-15、S-FBR-16：实例挂载及开始释放时发送的同步通知。
///
/// 参数为相应的 [`Fiber`]，其 [`Fiber::is_alive`] 标识登记或撤回阶段。
/// 监听器的输出类型为 `()`。
pub struct InternalPlugin;
impl Event for InternalPlugin {
    const NAME: &'static str = "internal/plugin";
    type Args = Fiber;
    type Output = ();
    type Kind = SyncKind;
}
/// S-FBR-19：保存更新配置之前发送的同步通知。
///
/// 参数为实例句柄及新的原始配置 `(Fiber, RawConfig)`，监听器的输出类型为 `()`。
pub struct InternalCommit;
impl Event for InternalCommit {
    const NAME: &'static str = "internal/commit";
    type Args = (Fiber, RawConfig);
    type Output = ();
    type Kind = SyncKind;
}
/// S-REG-7：组件配置解析的瀑布事件。
///
/// 输入为 `(Fiber, RawConfig)`，输出为装入 [`RawConfig`] 的解析结果。
/// 监听器通过 `Next::run` 进入下一层，默认末端调用组件的配置解析函数。
/// 返回值的具体类型应与当前组件的解析配置类型一致。
pub struct InternalConfig;
impl Event for InternalConfig {
    const NAME: &'static str = "internal/config";
    type Args = (Fiber, RawConfig);
    type Output = RawConfig;
    type Kind = WaterfallKind;
}
/// S-FBR-19、S-FBR-20：活跃目标接收配置更新时的瀑布事件。
///
/// 输入为解析后的 [`RawConfig`]，输出为等待更新结果的 `Task<Result>`。
/// 当前实例的监听器及全局监听器参与派发。`Next::run` 进入下一层，默认末端
/// 调用 [`Fiber::restart`]；监听器也可返回自身的任务来接管更新。
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
    /// S-FBR-4：返回自身及祖先中最近一个具名组件的名称，缺省为 `root`。
    pub fn name(&self) -> String {
        name_locked(&self.kernel.state.lock(), self.id)
    }
    /// 返回最近一次记录的生命周期状态。
    pub fn state(&self) -> FiberState {
        self.kernel.state.lock().fibers[self.id].state
    }
    /// 返回调和器最近计算的目标纪元。
    ///
    /// 进行中的加载以开始时的纪元运行，目标变化驱动随后串联的迁移。
    pub fn epoch(&self) -> Epoch {
        self.kernel.state.lock().fibers[self.id].target.clone()
    }
    /// S-FBR-3：实例的挂载效应仍然存活时返回 `true`。
    ///
    /// 释放开始后返回 `false`。该值独立于依赖满足状态与迁移阶段；根纤程保持存活。
    pub fn is_alive(&self) -> bool {
        self.kernel.state.lock().fibers[self.id].alive
    }
    /// 查询本实例的调和请求和迁移是否已全部结束。
    ///
    /// 返回值对应 `queued == false`、`inertia == false`、`notifications == 0`
    /// 且 `suspended.is_none()`：排队请求、迁移、状态传播及保留待重新提交的任务均已处理。
    /// `Pending` 和清理已结束的 `Failed` 状态也可达到稳定。
    pub fn is_settled(&self) -> bool {
        let state = self.kernel.state.lock();
        let data = &state.fibers[self.id];
        !data.inertia && !data.queued && data.notifications == 0 && data.suspended.is_none()
    }
    /// T-LIF-9：返回该实例自身的上下文，效应归属于当前实例。
    pub fn context(&self) -> Context {
        Context {
            kernel: self.kernel.clone(),
            scope: self.scope.clone(),
        }
    }
    /// 返回当前组件定义的身份。
    ///
    /// 根纤程及已完成释放、回收实现记录的实例返回 `None`。
    pub fn component_id(&self) -> Option<ComponentId> {
        self.kernel.state.lock().fibers[self.id]
            .component
            .as_ref()
            .map(|component| component.id)
    }
    /// 按具体类型 `R` 读取实例保存的原始配置。
    ///
    /// 类型匹配时返回共享的 `Arc<R>`，类型差异返回 `None`；释放完成后保存值为 `()`。
    pub fn config<R: Send + Sync + 'static>(&self) -> Option<Arc<R>> {
        self.kernel.state.lock().fibers[self.id]
            .config
            .clone()
            .downcast()
            .ok()
    }
    /// 返回当前依赖规格的快照；组件替换后反映新定义的声明。
    pub fn inject(&self) -> Inject {
        self.kernel.state.lock().fibers[self.id].inject.clone()
    }
    /// S-EFF-7：按登记顺序返回顶层效应的元信息树。
    ///
    /// 经 `EffectScope::adopt` 归属到父效应的子项列在父项的 `children` 中。
    pub fn effects(&self) -> Vec<EffectMeta> {
        let state = self.kernel.state.lock();
        state.fibers[self.id]
            .effects
            .iter()
            .map(|id| state.effects[*id].meta(&state))
            .collect()
    }

    /// S-FBR-14、S-FBR-21：等待当前及随后串联的迁移完成。
    ///
    /// 排队调和也属于等待范围。依赖欠缺而稳定于 `Pending` 时可返回 `Ok(())`。
    /// 加载失败返回对应错误；宿主拒收造成清理任务保留时返回派发错误，
    /// 随后的 [`Fiber::dispose`] 可恢复这些任务。
    /// 内核通过宿主的 `Spawn` 推进迁移，返回的 Future 承担结果等待。
    pub fn settled(&self) -> Task<Result> {
        self.wait(true)
    }
    pub(crate) fn wait(&self, report_error: bool) -> Task<Result> {
        self.wait_for(report_error, true)
    }

    pub(crate) fn wait_completion(&self) -> Task<Result> {
        self.wait_for(false, false)
    }

    fn wait_for(&self, report_error: bool, report_dispatch: bool) -> Task<Result> {
        let fiber = self.clone();
        Box::pin(poll_fn(move |cx| {
            let mut state = fiber.kernel.state.lock();
            if report_dispatch && let Some(error) = dispatch_error(&state, fiber.id) {
                return Poll::Ready(Err(error));
            }
            let data = &mut state.fibers[fiber.id];
            if !data.inertia && !data.queued && data.notifications == 0 && data.suspended.is_none()
            {
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
    ///
    /// 调用时同步增加修订号并提交调和请求，返回的任务等待迁移稳定。
    /// 已加载实例先卸载再加载，依赖欠缺时保持 `Pending`。
    /// 失活实例或保留清理的派发错误通过返回任务交回；加载失败的恢复入口为
    /// [`Fiber::update`] 或 [`crate::Registry::replace`]。
    pub fn restart(&self) -> Task<Result> {
        {
            let mut state = self.kernel.state.lock();
            let data = &mut state.fibers[self.id];
            if !data.alive {
                return Box::pin(async { Err(Error::inactive()) });
            }
            if let Some(error) = &data.dispatch_error {
                let error = error.clone();
                return Box::pin(async move { Err(error) });
            }
            data.revision += 1;
            enqueue_locked(&mut state, self.id);
        }
        drain(&self.kernel);
        self.settled()
    }

    /// S-FBR-19、S-FBR-20：保留原始配置，将解析结果交给更新监听器。
    ///
    /// `R` 应与当前组件接受的原始配置类型一致。目标处于活跃纪元时先解析新值，
    /// 校验失败则保留当前配置及效应。通过 [`InternalCommit`] 通知后保存原始值，
    /// 清除最近的加载错误，再由 [`InternalUpdate`] 监听器或默认重启流程处理更新。
    /// 走重启流程的连续更新按最新配置调和，返回的任务交回更新结果。
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
    ///
    /// 调用时立即发起操作，返回的任务等待释放结果。子实例撤回挂载效应并在清理后
    /// 进入 `Disposed`；根纤程通过 [`Fiber::restart`] 回收自身效应后回到 `Active`。
    /// 重复调用会恢复组件树内由宿主拒收而保留的清理任务；已结束的子实例释放保持幂等。
    /// 清理回调错误写入日志，其余清理继续执行；派发受阻时向等待者交回错误。
    pub fn dispose(&self) -> Task<Result> {
        resume_cleanup(&self.kernel, self.id);
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
            effect.begin_dispose();
            let task = effect.disposal_result();
            let fiber = self.clone();
            Box::pin(async move {
                fiber.wait(false).await?;
                task.await
            })
        } else {
            self.wait(false)
        }
    }

    pub(crate) fn begin_dispose(&self) {
        let runtime = {
            let mut state = self.kernel.state.lock();
            state.fibers[self.id].alive = false;
            state.fibers[self.id].lifetime = None;
            enqueue_locked(&mut state, self.id);
            registry::unregister(&mut state, self.id)
        };
        drop(runtime);
        self.emit_plugin();
        drain(&self.kernel);
    }

    pub(crate) fn emit_plugin(&self) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
            self.context().emit::<InternalPlugin>(self)
        })) {
            self.kernel.report(self.id, Error::panic(payload));
        }
    }
}

impl Context {
    /// T-LIF-7：组件在异步步骤边界检查其加载纪元。
    ///
    /// 实例存活且最近计算的目标纪元与当前加载纪元相等时返回 `true`。
    /// 组件可在值变为 `false` 后结束当前执行，由卸载流程恢复已经登记的效应。
    pub fn is_current(&self) -> bool {
        let state = self.kernel.state.lock();
        let data = &state.fibers[self.scope.fiber];
        data.alive && data.target == data.running_epoch
    }
}

/// A-5、S-FBR-14：输入更新与排队标记同锁完成，等待者观察完整的调和请求。
pub(crate) fn enqueue_locked(state: &mut State, id: FiberId) {
    let data = &mut state.fibers[id];
    if data.queued {
        return;
    }
    if let Some(scope) = &data.scope {
        data.queued = true;
        state.dirty.push_back(scope.clone());
    }
}

/// T-LIF-3、A-5：唯一调和者消费队列，动作沿用队列持有的上下文租约。
pub(crate) fn drain(kernel: &Arc<Kernel>) {
    {
        let mut state = kernel.state.lock();
        if state.draining {
            return;
        }
        state.draining = true;
    }
    loop {
        let action = {
            let mut state = kernel.state.lock();
            let Some(scope) = state.dirty.pop_front() else {
                state.draining = false;
                return;
            };
            let id = scope.fiber;
            let fiber = Fiber {
                kernel: kernel.clone(),
                id,
                scope,
            };
            let data = &state.fibers[id];
            let target = if data.error.is_some() && data.alive {
                data.target.clone()
            } else if data.alive {
                crate::reflect::epoch_ids(&state, id).map_or(Epoch::Inactive, |impls| {
                    Epoch::Active {
                        revision: data.revision,
                        impls,
                    }
                })
            } else {
                Epoch::Inactive
            };
            let data = &mut state.fibers[id];
            data.queued = false;
            let old_target = std::mem::replace(&mut data.target, target);
            let old_state = data.state;
            let transition = if data.inertia || data.suspended.is_some() {
                None
            } else if data.target == old_target {
                data.state = data.idle_state();
                None
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
                Some(loading)
            };
            let new_state = data.state;
            if old_state != new_state {
                data.notifications += 1;
            }
            if transition == Some(true) {
                crate::reflect::commit_locked(&mut state, id);
            }
            (fiber, old_state, new_state, transition)
        };
        let (fiber, old, new, transition) = action;
        status_changed(&fiber, old, new);
        if let Some(loading) = transition {
            let task_fiber = fiber.clone();
            let task = Box::pin(async move {
                transition_loop(task_fiber, loading).await;
            });
            submit_transition(&fiber, task, loading);
        } else if new == FiberState::Disposed {
            release_record(&fiber);
        }
        wake_stable(&fiber);
    }
}

fn status_changed(fiber: &Fiber, old: FiberState, new: FiberState) {
    if old == new {
        return;
    }
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
        fiber
            .context()
            .emit::<InternalStatus>(&(fiber.clone(), old))
    })) {
        fiber.kernel.report(fiber.id, Error::panic(payload));
    }
    if old == FiberState::Active || new == FiberState::Active {
        crate::reflect::status_changed(&fiber.kernel, fiber.id);
    }
    fiber.kernel.state.lock().fibers[fiber.id].notifications -= 1;
}

/// T-LIF-4、T-LIF-5、T-LIF-6：迁移结束后串联到最新目标纪元。
async fn transition_loop(fiber: Fiber, mut loading: bool) {
    let kernel = fiber.kernel.clone();
    let id = fiber.id;
    loop {
        if loading {
            let epoch = kernel.state.lock().fibers[id].running_epoch.clone();
            yield_once().await;
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
                    kernel.report(id, error.clone());
                    let mut state = kernel.state.lock();
                    let data = &mut state.fibers[id];
                    if data.target == epoch {
                        data.error = Some(error);
                        data.target = Epoch::Inactive;
                    }
                }
            }
        } else {
            crate::reflect::wait_dependents(kernel.clone(), id).await;
            // S-FBR-12：让已排队的加载推进，再开始顶层效应的清理调用。
            yield_once().await;
            let effects = {
                let mut state = kernel.state.lock();
                let ids = state.fibers[id].effects.take();
                ids.into_iter()
                    .rev()
                    .map(|id| Effect::handle(&kernel, id, &mut state))
                    .collect::<Vec<_>>()
            };
            let tasks = effects
                .iter()
                .map(|effect| {
                    effect.begin_dispose();
                    effect.completion()
                })
                .collect::<Vec<_>>();
            for result in join_all(tasks).await {
                if let Err(error) = result {
                    kernel.report(id, error);
                }
            }
            crate::reflect::release_committed(&kernel, id);
        }
        let Some(next) = finish_transition(&fiber, loading) else {
            if fiber.state() == FiberState::Disposed {
                release_record(&fiber);
            }
            return;
        };
        loading = next;
    }
}

/// S-FBR-2、S-FBR-14：状态传播完成后发布稳定结果，并处理通知回调改变的目标。
fn finish_transition(fiber: &Fiber, loading: bool) -> Option<bool> {
    loop {
        let (old, new, next) = {
            let mut state = fiber.kernel.state.lock();
            let data = &mut state.fibers[fiber.id];
            let next = if loading {
                (data.target != data.running_epoch).then_some(false)
            } else {
                (data.target != Epoch::Inactive).then_some(true)
            };
            let old = data.state;
            data.state = match next {
                Some(true) => {
                    data.running_epoch = data.target.clone();
                    FiberState::Loading
                }
                Some(false) => FiberState::Unloading,
                None => data.idle_state(),
            };
            let new = data.state;
            if old != new {
                data.notifications += 1;
            } else if next.is_none() {
                data.inertia = false;
            }
            if next == Some(true) {
                crate::reflect::commit_locked(&mut state, fiber.id);
            }
            (old, new, next)
        };
        status_changed(fiber, old, new);
        if let Some(next) = next {
            return Some(next);
        }
        if old == new {
            wake_stable(fiber);
            return None;
        }
    }
}

fn wake_stable(fiber: &Fiber) {
    let waiters = {
        let mut state = fiber.kernel.state.lock();
        let data = &mut state.fibers[fiber.id];
        if data.queued || data.inertia || data.notifications != 0 || data.suspended.is_some() {
            return;
        }
        std::mem::take(&mut data.waiters)
    };
    for waiter in waiters {
        waiter.wake();
    }
}

async fn yield_once() {
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
}

pub(crate) fn name_locked(state: &State, mut id: FiberId) -> String {
    loop {
        let data = &state.fibers[id];
        if !data.name.is_empty() {
            return data.name.into();
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
        if data.inertia || data.queued || data.notifications != 0 || data.suspended.is_some() {
            return;
        }
        (
            data.scope.take(),
            data.component.take(),
            std::mem::replace(&mut data.config, Arc::new(())),
        )
    };
    drop(removed);
}

/// 派发失败保留已经开始清理的任务；一次重新提交耗尽后由显式释放继续推进。
fn submit_transition(fiber: &Fiber, task: Task, loading: bool) {
    let Err(failure) = fiber.kernel.submit(task) else {
        return;
    };
    fiber.kernel.report(fiber.id, failure.error.clone());
    let Some(task) = failure.rejected else {
        return;
    };
    if !loading {
        submit_cleanup(fiber, task);
        return;
    }
    drop(task);
    let old = {
        let mut state = fiber.kernel.state.lock();
        let data = &mut state.fibers[fiber.id];
        let old = data.state;
        data.error = Some(failure.error);
        data.target = Epoch::Inactive;
        data.state = FiberState::Unloading;
        if old != data.state {
            data.notifications += 1;
        }
        old
    };
    status_changed(fiber, old, FiberState::Unloading);
    let cleanup = fiber.clone();
    submit_cleanup(
        fiber,
        Box::pin(async move {
            transition_loop(cleanup, false).await;
        }),
    );
}

fn submit_cleanup(fiber: &Fiber, task: Task) {
    let Err(failure) = fiber.kernel.submit(task) else {
        return;
    };
    fiber.kernel.report(fiber.id, failure.error.clone());
    let Some(task) = failure.rejected else {
        return;
    };
    let old = {
        let mut state = fiber.kernel.state.lock();
        let data = &mut state.fibers[fiber.id];
        data.suspended = Some(task);
        data.dispatch_error = Some(failure.error);
        data.inertia = false;
        let old = std::mem::replace(&mut data.state, FiberState::Failed);
        if old != data.state {
            data.notifications += 1;
        }
        old
    };
    status_changed(fiber, old, FiberState::Failed);
    wake_dispatch_waiters(&fiber.kernel);
}

fn descendant(state: &State, mut id: FiberId, ancestor: FiberId) -> bool {
    loop {
        if id == ancestor {
            return true;
        }
        let Some(parent) = &state.fibers[id].parent_scope else {
            return false;
        };
        id = parent.fiber;
    }
}

fn dispatch_error(state: &State, id: FiberId) -> Option<Error> {
    state
        .fibers
        .iter()
        .find_map(|(current, data)| {
            data.dispatch_error
                .as_ref()
                .filter(|_| descendant(state, current, id))
                .cloned()
        })
        .or_else(|| {
            state.effects.values().find_map(|data| {
                data.dispatch_error
                    .as_ref()
                    .filter(|_| descendant(state, data.owner, id))
                    .cloned()
            })
        })
}

pub(crate) fn wake_dispatch_waiters(kernel: &Arc<Kernel>) {
    let waiters = {
        let mut state = kernel.state.lock();
        state
            .fibers
            .values_mut()
            .flat_map(|fiber| std::mem::take(&mut fiber.waiters))
            .collect::<Vec<_>>()
    };
    for waiter in waiters {
        waiter.wake();
    }
}

/// 显式释放恢复本组件树此前已请求且由宿主拒收的清理。
pub(crate) fn resume_cleanup(kernel: &Arc<Kernel>, id: FiberId) {
    let (fibers, effects) = {
        let mut state = kernel.state.lock();
        let ids = state
            .fibers
            .iter()
            .filter_map(|(current, data)| {
                (data.suspended.is_some() && descendant(&state, current, id)).then_some(current)
            })
            .collect::<Vec<_>>();
        let fibers = ids
            .into_iter()
            .map(|id| {
                let fiber = Fiber::handle(kernel, id, &state);
                let data = &mut state.fibers[id];
                let task = data.suspended.take().expect("suspended cleanup selected");
                data.dispatch_error = None;
                data.inertia = true;
                let old = std::mem::replace(&mut data.state, FiberState::Unloading);
                if old != data.state {
                    data.notifications += 1;
                }
                (fiber, task, old)
            })
            .collect::<Vec<_>>();
        let ids = state
            .effects
            .iter()
            .filter_map(|(current, data)| {
                (data.suspended.is_some() && descendant(&state, data.owner, id)).then_some(current)
            })
            .collect::<Vec<_>>();
        let effects = ids
            .into_iter()
            .map(|id| Effect::handle(kernel, id, &mut state))
            .collect::<Vec<_>>();
        (fibers, effects)
    };
    for (fiber, task, old) in fibers {
        status_changed(&fiber, old, FiberState::Unloading);
        submit_transition(&fiber, task, false);
    }
    for effect in effects {
        effect.resume_cleanup();
    }
}
