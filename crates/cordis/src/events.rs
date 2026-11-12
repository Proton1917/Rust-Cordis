//! 类型化事件、可回退监听器与快照派发。

use crate::{
    AnyArc, Context, Effect, Error, Result, Task,
    context::Scope,
    kernel::{EffectId, Kernel},
};
use futures_util::future::join_all;
use std::{
    any::{Any, TypeId},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// 事件类型决定参数、返回值与可读名称。
pub trait Event: 'static {
    /// 观察记录、效应标签与错误消息中使用的事件名称。
    const NAME: &'static str;
    /// 监听器接收的参数；异步与瀑布派发为各次调用克隆参数。
    type Args: Clone + Send + Sync + 'static;
    /// 单个监听器产生的结果类型。
    type Output: Send + 'static;
    /// 派发族：`SyncKind`、`AsyncKind` 或 `WaterfallKind`。
    type Kind;
}

/// 选择同步通知与首个结果派发。
pub struct SyncKind;
/// 选择并发等待与顺序等待派发。
pub struct AsyncKind;
/// 选择带 `Next` 延续的中间件派发。
pub struct WaterfallKind;

/// 同步监听器供 `emit` 与 `bail` 调用。
pub trait SyncEvent: Event<Kind = SyncKind> {}
impl<E: Event<Kind = SyncKind>> SyncEvent for E {}
/// 异步监听器供 `parallel` 与 `serial` 调用。
pub trait AsyncEvent: Event<Kind = AsyncKind> {}
impl<E: Event<Kind = AsyncKind>> AsyncEvent for E {}
/// 中间件监听器通过 `Next` 进入下一层。
pub trait WaterfallEvent: Event<Kind = WaterfallKind> {}
impl<E: Event<Kind = WaterfallKind>> WaterfallEvent for E {}

/// 监听器的登记顺序与过滤策略，两个选项初始均为 `false`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventOptions {
    /// 把本次登记插入当前事件监听器列表头部。
    pub prepend: bool,
    /// 使监听器直接通过派发过滤器。
    pub global: bool,
}

/// `InternalDispatch` 记录的派发方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchMode {
    /// 同步调用全部监听器。
    Emit,
    /// 同步取得首个 `Some`。
    Bail,
    /// 并发等待全部监听器。
    Parallel,
    /// 逐个等待并取得首个 `Some`。
    Serial,
    /// 通过各层延续嵌套调用。
    Waterfall,
}

/// 可共享的同步监听器；返回 `Some` 可结束 `bail`。
pub type SyncListener<E> =
    Arc<dyn Fn(&<E as Event>::Args) -> Option<<E as Event>::Output> + Send + Sync>;
/// 可共享的异步监听器；任务结果携带业务返回值或调用失败。
pub type AsyncListener<E> =
    Arc<dyn Fn(<E as Event>::Args) -> Task<Result<Option<<E as Event>::Output>>> + Send + Sync>;
/// 可共享的中间件监听器，通过 `Next::run` 进入下一层。
pub type WaterfallListener<E> =
    Arc<dyn Fn(<E as Event>::Args, Next<E>) -> Result<<E as Event>::Output> + Send + Sync>;

struct SyncCallback<E: Event>(SyncListener<E>);
struct AsyncCallback<E: Event>(AsyncListener<E>);
struct WaterfallCallback<E: WaterfallEvent>(WaterfallListener<E>);

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum EventId {
    Typed(TypeId),
    Named(Arc<str>, bool),
}

impl EventId {
    fn of<E: Event>() -> Self {
        Self::Typed(TypeId::of::<E>())
    }
    fn is_named(&self) -> bool {
        matches!(self, Self::Named(..))
    }
}

struct NamedSync;
impl Event for NamedSync {
    const NAME: &'static str = "";
    type Args = AnyArc;
    type Output = Error;
    type Kind = SyncKind;
}
struct NamedAsync;
impl Event for NamedAsync {
    const NAME: &'static str = "";
    type Args = AnyArc;
    type Output = AnyArc;
    type Kind = AsyncKind;
}

pub(crate) struct Hook {
    scope: Arc<Scope>,
    callback: Arc<dyn Any + Send + Sync>,
    options: EventOptions,
    once: bool,
    removed: AtomicBool,
    effect: EffectId,
}

impl Hook {
    /// S-EVT-2、S-EVT-9：一次性监听器先移除，再执行用户回调。
    fn claim(&self, kernel: &Arc<Kernel>) -> bool {
        if self.once {
            if self.removed.swap(true, Ordering::AcqRel) {
                return false;
            }
            let effect = {
                let mut state = kernel.state.lock();
                if !state.effects.contains_key(self.effect) {
                    return false;
                }
                Effect::handle(kernel, self.effect, &mut state)
            };
            drop(effect.dispose());
            true
        } else {
            !self.removed.load(Ordering::Acquire)
        }
    }
}

/// `internal/listener` 接收的登记请求，可由监听器接管为另一个效应。
#[derive(Clone)]
pub struct ListenerRegistration {
    /// 发起登记的上下文，同时决定效应归属。
    pub context: Context,
    /// 事件声明的可读名称。
    pub name: Arc<str>,
    /// 原始登记选项。
    pub options: EventOptions,
    /// 本次请求是否在首次调用前撤销登记。
    pub once: bool,
    event: EventId,
    callback: Arc<dyn Any + Send + Sync>,
}

impl ListenerRegistration {
    /// 判断请求的事件身份是否为 `E`。
    pub fn is<E: Event>(&self) -> bool {
        self.event == EventId::of::<E>()
    }

    /// K-3：返回本次请求是否采用命名事件入口。
    pub fn is_named(&self) -> bool {
        self.event.is_named()
    }

    /// 类型匹配时取得同步监听器，否则返回 `None`。
    pub fn sync<E: SyncEvent>(&self) -> Option<SyncListener<E>> {
        self.callback
            .downcast_ref::<SyncCallback<E>>()
            .map(|callback| callback.0.clone())
    }

    /// 类型匹配时取得异步监听器，否则返回 `None`。
    pub fn asynchronous<E: AsyncEvent>(&self) -> Option<AsyncListener<E>> {
        self.callback
            .downcast_ref::<AsyncCallback<E>>()
            .map(|callback| callback.0.clone())
    }

    /// 类型匹配时取得瀑布监听器，否则返回 `None`。
    pub fn waterfall<E: WaterfallEvent>(&self) -> Option<WaterfallListener<E>> {
        self.callback
            .downcast_ref::<WaterfallCallback<E>>()
            .map(|callback| callback.0.clone())
    }

    /// 在接管回调中按给定选项登记，返回其可回退效应。
    /// 直接登记请求中的回调，保留原上下文和一次性属性。
    #[track_caller]
    pub fn register(&self, options: EventOptions) -> Result<Effect> {
        let mut request = self.clone();
        request.options = options;
        self.context.register_hook(request)
    }
}

/// 监听器登记前的同步询问事件；首个 `Some(Effect)` 接管该次登记。
pub struct InternalListener;
impl Event for InternalListener {
    const NAME: &'static str = "internal/listener";
    type Args = ListenerRegistration;
    type Output = Effect;
    type Kind = SyncKind;
}

/// 派发观察记录保留事件类型、参数与发起上下文。
#[derive(Clone)]
pub struct Dispatch {
    /// 本次调用采用的派发方式。
    pub mode: DispatchMode,
    /// 被派发事件的可读名称。
    pub name: Arc<str>,
    /// 发起派发的上下文。
    pub context: Context,
    /// 本次派发是否携带过滤器。
    pub filtered: bool,
    /// 参数的拥有型快照，可通过 `args::<E>` 按事件类型读取。
    pub args: Arc<dyn Any + Send + Sync>,
    event: EventId,
}

impl Dispatch {
    /// 判断被派发的事件身份是否为 `E`。
    pub fn is<E: Event>(&self) -> bool {
        self.event == EventId::of::<E>()
    }

    /// K-3：命名派发的 `args` 保存传入的原始 `AnyArc` 载荷。
    pub fn is_named(&self) -> bool {
        self.event.is_named()
    }

    /// 事件类型匹配时返回参数引用，否则返回 `None`。
    pub fn args<E: Event>(&self) -> Option<&E::Args> {
        self.is::<E>()
            .then(|| self.args.downcast_ref::<E::Args>())
            .flatten()
    }
}

/// 业务事件取得监听器快照前的同步观察事件。
pub struct InternalDispatch;
impl Event for InternalDispatch {
    const NAME: &'static str = "internal/dispatch";
    type Args = Dispatch;
    type Output = ();
    type Kind = SyncKind;
}

impl Context {
    /// S-EVT-1：登记同步监听器，返回归属当前纤程的效应。
    /// 采用默认选项，回调接收参数引用并返回可选结果。
    #[track_caller]
    pub fn on<E: SyncEvent>(
        &self,
        callback: impl Fn(&E::Args) -> Option<E::Output> + Send + Sync + 'static,
    ) -> Result<Effect> {
        self.on_with::<E>(callback, EventOptions::default())
    }

    /// 按 `options` 登记同步监听器，返回可显式释放的效应。
    #[track_caller]
    pub fn on_with<E: SyncEvent>(
        &self,
        callback: impl Fn(&E::Args) -> Option<E::Output> + Send + Sync + 'static,
        options: EventOptions,
    ) -> Result<Effect> {
        self.listen::<E>(
            Arc::new(SyncCallback::<E>(Arc::new(callback))),
            options,
            false,
        )
    }

    /// 以默认选项登记一次性同步监听器，在调用回调前撤销登记。
    #[track_caller]
    pub fn once<E: SyncEvent>(
        &self,
        callback: impl Fn(&E::Args) -> Option<E::Output> + Send + Sync + 'static,
    ) -> Result<Effect> {
        self.once_with::<E>(callback, EventOptions::default())
    }

    /// 按 `options` 登记一次性同步监听器，递归派发共享消费状态。
    #[track_caller]
    pub fn once_with<E: SyncEvent>(
        &self,
        callback: impl Fn(&E::Args) -> Option<E::Output> + Send + Sync + 'static,
        options: EventOptions,
    ) -> Result<Effect> {
        self.listen::<E>(
            Arc::new(SyncCallback::<E>(Arc::new(callback))),
            options,
            true,
        )
    }

    /// 登记拥有参数的异步回调，返回监听器效应；任务由派发调用方等待。
    #[track_caller]
    pub fn on_async<E: AsyncEvent>(
        &self,
        callback: impl Fn(E::Args) -> Task<Result<Option<E::Output>>> + Send + Sync + 'static,
        options: EventOptions,
    ) -> Result<Effect> {
        self.listen::<E>(
            Arc::new(AsyncCallback::<E>(Arc::new(callback))),
            options,
            false,
        )
    }

    /// 登记一次性异步回调，并发派发共同消费同一个登记。
    #[track_caller]
    pub fn once_async<E: AsyncEvent>(
        &self,
        callback: impl Fn(E::Args) -> Task<Result<Option<E::Output>>> + Send + Sync + 'static,
        options: EventOptions,
    ) -> Result<Effect> {
        self.listen::<E>(
            Arc::new(AsyncCallback::<E>(Arc::new(callback))),
            options,
            true,
        )
    }

    /// 登记中间件回调，回调通过 `Next` 取得后继结果或直接返回自身结果。
    #[track_caller]
    pub fn on_waterfall<E: WaterfallEvent>(
        &self,
        callback: impl Fn(E::Args, Next<E>) -> Result<E::Output> + Send + Sync + 'static,
        options: EventOptions,
    ) -> Result<Effect> {
        self.listen::<E>(
            Arc::new(WaterfallCallback::<E>(Arc::new(callback))),
            options,
            false,
        )
    }

    /// 登记一次性中间件，在进入回调前撤销登记。
    #[track_caller]
    pub fn once_waterfall<E: WaterfallEvent>(
        &self,
        callback: impl Fn(E::Args, Next<E>) -> Result<E::Output> + Send + Sync + 'static,
        options: EventOptions,
    ) -> Result<Effect> {
        self.listen::<E>(
            Arc::new(WaterfallCallback::<E>(Arc::new(callback))),
            options,
            true,
        )
    }

    /// S-EVT-10：登记前按顺序询问接管监听器。
    #[track_caller]
    fn listen<E: Event>(
        &self,
        callback: Arc<dyn Any + Send + Sync>,
        options: EventOptions,
        once: bool,
    ) -> Result<Effect> {
        self.listen_event(EventId::of::<E>(), E::NAME.into(), callback, options, once)
    }

    /// K-3、S-EVT-1：按运行时名称登记同步监听器。
    #[track_caller]
    pub fn on_named(
        &self,
        name: impl Into<Arc<str>>,
        callback: impl Fn(&AnyArc) -> Result<Option<AnyArc>> + Send + Sync + 'static,
    ) -> Result<Effect> {
        self.on_named_with(name, callback, EventOptions::default())
    }

    /// 按给定选项登记命名同步监听器。
    #[track_caller]
    pub fn on_named_with(
        &self,
        name: impl Into<Arc<str>>,
        callback: impl Fn(&AnyArc) -> Result<Option<AnyArc>> + Send + Sync + 'static,
        options: EventOptions,
    ) -> Result<Effect> {
        let name = name.into();
        self.listen_event(
            EventId::Named(name.clone(), false),
            name,
            Arc::new(SyncCallback::<NamedSync>(Arc::new(move |args| {
                callback(args).err()
            }))),
            options,
            false,
        )
    }

    /// S-EVT-9、K-3：在首次调用前撤销命名同步监听器，支持递归派发。
    #[track_caller]
    pub fn once_named(
        &self,
        name: impl Into<Arc<str>>,
        callback: impl Fn(&AnyArc) -> Result<Option<AnyArc>> + Send + Sync + 'static,
    ) -> Result<Effect> {
        let name = name.into();
        self.listen_event(
            EventId::Named(name.clone(), false),
            name,
            Arc::new(SyncCallback::<NamedSync>(Arc::new(move |args| {
                callback(args).err()
            }))),
            EventOptions::default(),
            true,
        )
    }

    /// K-3、S-EVT-5：登记命名异步监听器，任务由派发调用方等待。
    #[track_caller]
    pub fn on_named_async(
        &self,
        name: impl Into<Arc<str>>,
        callback: impl Fn(AnyArc) -> Task<Result<Option<AnyArc>>> + Send + Sync + 'static,
        options: EventOptions,
    ) -> Result<Effect> {
        let name = name.into();
        self.listen_event(
            EventId::Named(name.clone(), true),
            name,
            Arc::new(AsyncCallback::<NamedAsync>(Arc::new(callback))),
            options,
            false,
        )
    }

    #[track_caller]
    fn listen_event(
        &self,
        event: EventId,
        name: Arc<str>,
        callback: AnyArc,
        options: EventOptions,
        once: bool,
    ) -> Result<Effect> {
        if !self.kernel.state.lock().fibers[self.scope.fiber].alive {
            return Err(Error::inactive().at(std::panic::Location::caller()));
        }
        let request = ListenerRegistration {
            context: self.clone(),
            name,
            options,
            once,
            event,
            callback,
        };
        if let Some(effect) = self.bail::<InternalListener>(&request) {
            return Ok(effect);
        }
        self.register_hook(request)
    }

    #[track_caller]
    fn register_hook(&self, request: ListenerRegistration) -> Result<Effect> {
        let kernel = Arc::downgrade(&self.kernel);
        self.effect(format!("ctx.on(\"{}\")", request.name), move |fx| {
            let hook = Arc::new(Hook {
                scope: request.context.scope.clone(),
                callback: request.callback,
                options: request.options,
                once: request.once,
                removed: AtomicBool::new(false),
                effect: fx.effect_id(),
            });
            {
                let mut state = request.context.kernel.state.lock();
                let named = request.event.is_named();
                if state
                    .event_names
                    .get(&request.name)
                    .is_some_and(|old| *old != named)
                {
                    return Err(Error::new(format!(
                        "event name \"{}\" belongs to another key",
                        request.name
                    )));
                }
                state.event_names.insert(request.name, named);
                let hooks = state.hooks.entry(request.event.clone()).or_default();
                if hook.options.prepend {
                    hooks.insert(0, hook.clone());
                } else {
                    hooks.push(hook.clone());
                }
            }
            fx.defer(move || {
                hook.removed.store(true, Ordering::Release);
                if let Some(kernel) = kernel.upgrade() {
                    let removed = {
                        let mut state = kernel.state.lock();
                        let hooks = state
                            .hooks
                            .get_mut(&request.event)
                            .expect("registered event");
                        let index = hooks
                            .iter()
                            .position(|item| Arc::ptr_eq(item, &hook))
                            .expect("registered hook");
                        let removed = hooks.remove(index);
                        if hooks.is_empty() {
                            state.hooks.remove(&request.event);
                        }
                        removed
                    };
                    drop(removed);
                }
                Ok(())
            })
        })
    }

    /// A-4、S-EVT-3、S-EVT-12：锁内固定快照，锁外观察派发并应用过滤。
    fn event_snapshot<E: Event>(
        &self,
        mode: DispatchMode,
        args: &E::Args,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) -> Vec<Arc<Hook>> {
        self.event_snapshot_key(
            EventId::of::<E>(),
            E::NAME.into(),
            mode,
            || Arc::new(args.clone()),
            filter,
        )
    }

    fn event_snapshot_key(
        &self,
        event: EventId,
        name: Arc<str>,
        mode: DispatchMode,
        args: impl FnOnce() -> AnyArc,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) -> Vec<Arc<Hook>> {
        if !name.starts_with("internal/") {
            let observed = self
                .kernel
                .state
                .lock()
                .hooks
                .contains_key(&EventId::of::<InternalDispatch>());
            if observed {
                self.emit::<InternalDispatch>(&Dispatch {
                    mode,
                    name,
                    context: self.clone(),
                    filtered: filter.is_some(),
                    args: args(),
                    event: event.clone(),
                });
            }
        }
        let mut hooks = self
            .kernel
            .state
            .lock()
            .hooks
            .get(&event)
            .cloned()
            .unwrap_or_default();
        if let Some(filter) = filter {
            hooks.retain(|hook| {
                hook.options.global
                    || filter(&Context {
                        kernel: self.kernel.clone(),
                        scope: hook.scope.clone(),
                    })
            });
        }
        hooks
    }

    /// S-EVT-4、A-9：同步调用；panic 由调用方接收。
    pub fn emit<E: SyncEvent>(&self, args: &E::Args) {
        self.emit_filtered::<E>(args, None);
    }

    /// 按登记上下文筛选后同步派发；全局监听器直接通过过滤。
    pub fn emit_filtered<E: SyncEvent>(
        &self,
        args: &E::Args,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) {
        self.emit_hooks::<E>(
            self.event_snapshot::<E>(DispatchMode::Emit, args, filter),
            args,
        );
    }

    /// K-3、S-EVT-4：同步派发命名事件，首个监听器错误交回调用方。
    pub fn emit_named(&self, name: impl Into<Arc<str>>, args: &AnyArc) -> Result {
        let name = name.into();
        let hooks = self.event_snapshot_key(
            EventId::Named(name.clone(), false),
            name,
            DispatchMode::Emit,
            || args.clone(),
            None,
        );
        self.bail_hooks::<NamedSync>(hooks, args)
            .map_or(Ok(()), Err)
    }

    fn emit_hooks<E: SyncEvent>(&self, hooks: Vec<Arc<Hook>>, args: &E::Args) {
        for hook in hooks {
            if hook.claim(&self.kernel) {
                (hook
                    .callback
                    .downcast_ref::<SyncCallback<E>>()
                    .expect("event callback type")
                    .0)(args);
            }
        }
    }

    /// S-EVT-6：首个 `Some` 结束同步派发。
    pub fn bail<E: SyncEvent>(&self, args: &E::Args) -> Option<E::Output> {
        self.bail_filtered::<E>(args, None)
    }

    /// 对过滤后的同步快照取得首个 `Some`，全部返回 `None` 时返回 `None`。
    pub fn bail_filtered<E: SyncEvent>(
        &self,
        args: &E::Args,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) -> Option<E::Output> {
        self.bail_hooks::<E>(
            self.event_snapshot::<E>(DispatchMode::Bail, args, filter),
            args,
        )
    }

    fn bail_hooks<E: SyncEvent>(&self, hooks: Vec<Arc<Hook>>, args: &E::Args) -> Option<E::Output> {
        for hook in hooks {
            if hook.claim(&self.kernel)
                && let Some(value) = (hook
                    .callback
                    .downcast_ref::<SyncCallback<E>>()
                    .expect("event callback type")
                    .0)(args)
            {
                return Some(value);
            }
        }
        None
    }

    /// S-EVT-5：监听器共同推进，全部结束后汇总错误。
    /// 调用时取得快照，返回任务在轮询时调用监听器；错误保留全部失败成员。
    pub fn parallel<E: AsyncEvent>(&self, args: E::Args) -> Task<Result> {
        self.parallel_filtered::<E>(args, None)
    }

    /// 对过滤后的快照并发等待，返回完整派发结果。
    pub fn parallel_filtered<E: AsyncEvent>(
        &self,
        args: E::Args,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) -> Task<Result> {
        let hooks = self.event_snapshot::<E>(DispatchMode::Parallel, &args, filter);
        self.parallel_hooks::<E>(hooks, args, E::NAME.into())
    }

    /// K-3、S-EVT-5：并发等待命名事件的异步监听器，汇总全部错误。
    pub fn parallel_named(&self, name: impl Into<Arc<str>>, args: AnyArc) -> Task<Result> {
        let name = name.into();
        let hooks = self.event_snapshot_key(
            EventId::Named(name.clone(), true),
            name.clone(),
            DispatchMode::Parallel,
            || args.clone(),
            None,
        );
        self.parallel_hooks::<NamedAsync>(hooks, args, name)
    }

    fn parallel_hooks<E: AsyncEvent>(
        &self,
        hooks: Vec<Arc<Hook>>,
        args: E::Args,
        name: Arc<str>,
    ) -> Task<Result> {
        let kernel = self.kernel.clone();
        Box::pin(async move {
            let results = join_all(hooks.into_iter().map(|hook| {
                let kernel = kernel.clone();
                let args = args.clone();
                async move {
                    if hook.claim(&kernel) {
                        (hook
                            .callback
                            .downcast_ref::<AsyncCallback<E>>()
                            .expect("event callback type")
                            .0)(args)
                        .await
                        .map(|_| ())
                    } else {
                        Ok(())
                    }
                }
            }))
            .await;
            let causes: Vec<_> = results
                .into_iter()
                .filter_map(std::result::Result::err)
                .collect();
            if causes.is_empty() {
                Ok(())
            } else {
                let mut error = Error::new(format!(
                    "one or more listeners of event \"{}\" failed",
                    name
                ));
                error.causes = causes;
                Err(error)
            }
        })
    }

    /// S-EVT-7：逐个等待，首个 `Some` 结束派发。
    /// 调用时取得快照，任务返回首个业务结果、`None` 或监听器错误。
    pub fn serial<E: AsyncEvent>(&self, args: E::Args) -> Task<Result<Option<E::Output>>> {
        self.serial_filtered::<E>(args, None)
    }

    /// 按登记上下文筛选后逐个等待，返回首个 `Some` 或失败。
    pub fn serial_filtered<E: AsyncEvent>(
        &self,
        args: E::Args,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) -> Task<Result<Option<E::Output>>> {
        let hooks = self.event_snapshot::<E>(DispatchMode::Serial, &args, filter);
        let kernel = self.kernel.clone();
        Box::pin(async move {
            for hook in hooks {
                if hook.claim(&kernel)
                    && let Some(value) = (hook
                        .callback
                        .downcast_ref::<AsyncCallback<E>>()
                        .expect("event callback type")
                        .0)(args.clone())
                    .await?
                {
                    return Ok(Some(value));
                }
            }
            Ok(None)
        })
    }

    /// S-EVT-8：每层拥有独立的一次性延续。
    /// `inner` 是最内层函数；返回值为异步任务时，由调用方继续等待该任务。
    pub fn waterfall<E: WaterfallEvent>(
        &self,
        args: E::Args,
        inner: impl Fn() -> Result<E::Output> + Send + Sync + 'static,
    ) -> Result<E::Output> {
        self.waterfall_filtered::<E>(args, inner, None)
    }

    /// 对过滤后的快照进行嵌套调用，以 `inner` 作为最内层函数。
    pub fn waterfall_filtered<E: WaterfallEvent>(
        &self,
        args: E::Args,
        inner: impl Fn() -> Result<E::Output> + Send + Sync + 'static,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) -> Result<E::Output> {
        Arc::new(WaterfallChain::<E> {
            hooks: self.event_snapshot::<E>(DispatchMode::Waterfall, &args, filter),
            kernel: self.kernel.clone(),
            args,
            inner: Box::new(inner),
        })
        .run(0)
    }

    /// 返回当前事件类型数量，供效应恢复快照使用。
    pub fn event_count(&self) -> usize {
        self.kernel.state.lock().hooks.len()
    }

    /// 返回事件类型 `E` 当前登记的监听器数量。
    pub fn listener_count<E: Event>(&self) -> usize {
        self.kernel
            .state
            .lock()
            .hooks
            .get(&EventId::of::<E>())
            .map_or(0, Vec::len)
    }
}

/// 共享同一调用权的中间件延续；第二次 `run` 返回错误。
pub struct Next<E: WaterfallEvent> {
    called: Arc<AtomicBool>,
    callback: Arc<dyn Fn() -> Result<E::Output> + Send + Sync>,
}

impl<E: WaterfallEvent> Clone for Next<E> {
    fn clone(&self) -> Self {
        Self {
            called: self.called.clone(),
            callback: self.callback.clone(),
        }
    }
}

impl<E: WaterfallEvent> Next<E> {
    /// 消费本层调用权并取得后继结果；再次调用或通过克隆重复调用返回错误。
    pub fn run(&self) -> Result<E::Output> {
        if self.called.swap(true, Ordering::AcqRel) {
            return Err(Error::new("next() called multiple times"));
        }
        (self.callback)()
    }
}

struct WaterfallChain<E: WaterfallEvent> {
    hooks: Vec<Arc<Hook>>,
    kernel: Arc<Kernel>,
    args: E::Args,
    inner: Box<dyn Fn() -> Result<E::Output> + Send + Sync>,
}

impl<E: WaterfallEvent> WaterfallChain<E> {
    fn run(self: Arc<Self>, mut index: usize) -> Result<E::Output> {
        while let Some(hook) = self.hooks.get(index) {
            index += 1;
            if !hook.claim(&self.kernel) {
                continue;
            }
            let chain = self.clone();
            let next = Next {
                called: Arc::new(AtomicBool::new(false)),
                callback: Arc::new(move || chain.clone().run(index)),
            };
            return (hook
                .callback
                .downcast_ref::<WaterfallCallback<E>>()
                .expect("event callback type")
                .0)(self.args.clone(), next);
        }
        (self.inner)()
    }
}
