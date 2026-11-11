//! 类型化事件、可回退监听器与快照派发。

use crate::{
    Context, Effect, Error, FiberState, Result, Task,
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
    const NAME: &'static str;
    type Args: Clone + Send + Sync + 'static;
    type Output: Send + 'static;
    type Kind;
}

pub struct SyncKind;
pub struct AsyncKind;
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventOptions {
    pub prepend: bool,
    pub global: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchMode {
    Emit,
    Bail,
    Parallel,
    Serial,
    Waterfall,
}

pub type SyncListener<E> =
    Arc<dyn Fn(&<E as Event>::Args) -> Option<<E as Event>::Output> + Send + Sync>;
pub type AsyncListener<E> =
    Arc<dyn Fn(<E as Event>::Args) -> Task<Result<Option<<E as Event>::Output>>> + Send + Sync>;
pub type WaterfallListener<E> =
    Arc<dyn Fn(<E as Event>::Args, Next<E>) -> Result<<E as Event>::Output> + Send + Sync>;

struct SyncCallback<E: Event>(SyncListener<E>);
struct AsyncCallback<E: Event>(AsyncListener<E>);
struct WaterfallCallback<E: WaterfallEvent>(WaterfallListener<E>);

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
    pub context: Context,
    pub name: &'static str,
    pub options: EventOptions,
    pub once: bool,
    event: TypeId,
    callback: Arc<dyn Any + Send + Sync>,
}

impl ListenerRegistration {
    pub fn is<E: Event>(&self) -> bool {
        self.event == TypeId::of::<E>()
    }

    pub fn sync<E: SyncEvent>(&self) -> Option<SyncListener<E>> {
        self.callback
            .downcast_ref::<SyncCallback<E>>()
            .map(|callback| callback.0.clone())
    }

    pub fn asynchronous<E: AsyncEvent>(&self) -> Option<AsyncListener<E>> {
        self.callback
            .downcast_ref::<AsyncCallback<E>>()
            .map(|callback| callback.0.clone())
    }

    pub fn waterfall<E: WaterfallEvent>(&self) -> Option<WaterfallListener<E>> {
        self.callback
            .downcast_ref::<WaterfallCallback<E>>()
            .map(|callback| callback.0.clone())
    }

    /// 在接管回调中按给定选项登记，返回其可回退效应。
    #[track_caller]
    pub fn register(&self, options: EventOptions) -> Result<Effect> {
        let mut request = self.clone();
        request.options = options;
        self.context.register_hook(request)
    }
}

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
    pub mode: DispatchMode,
    pub name: &'static str,
    pub context: Context,
    pub filtered: bool,
    pub args: Arc<dyn Any + Send + Sync>,
    event: TypeId,
}

impl Dispatch {
    pub fn is<E: Event>(&self) -> bool {
        self.event == TypeId::of::<E>()
    }

    pub fn args<E: Event>(&self) -> Option<&E::Args> {
        self.is::<E>()
            .then(|| self.args.downcast_ref::<E::Args>())
            .flatten()
    }
}

pub struct InternalDispatch;
impl Event for InternalDispatch {
    const NAME: &'static str = "internal/dispatch";
    type Args = Dispatch;
    type Output = ();
    type Kind = SyncKind;
}

impl Context {
    /// S-EVT-1：登记同步监听器，返回归属当前纤程的效应。
    #[track_caller]
    pub fn on<E: SyncEvent>(
        &self,
        callback: impl Fn(&E::Args) -> Option<E::Output> + Send + Sync + 'static,
    ) -> Result<Effect> {
        self.on_with::<E>(callback, EventOptions::default())
    }

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

    #[track_caller]
    pub fn once<E: SyncEvent>(
        &self,
        callback: impl Fn(&E::Args) -> Option<E::Output> + Send + Sync + 'static,
    ) -> Result<Effect> {
        self.once_with::<E>(callback, EventOptions::default())
    }

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
        if self.kernel.state.lock().fibers[self.scope.fiber].state == FiberState::Disposed {
            return Err(Error::inactive().at(std::panic::Location::caller()));
        }
        let request = ListenerRegistration {
            context: self.clone(),
            name: E::NAME,
            options,
            once,
            event: TypeId::of::<E>(),
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
                let hooks = state.hooks.entry(request.event).or_default();
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
        if !E::NAME.starts_with("internal/") {
            let observed = self
                .kernel
                .state
                .lock()
                .hooks
                .contains_key(&TypeId::of::<InternalDispatch>());
            if observed {
                self.emit::<InternalDispatch>(&Dispatch {
                    mode,
                    name: E::NAME,
                    context: self.clone(),
                    filtered: filter.is_some(),
                    args: Arc::new(args.clone()),
                    event: TypeId::of::<E>(),
                });
            }
        }
        let mut hooks = self
            .kernel
            .state
            .lock()
            .hooks
            .get(&TypeId::of::<E>())
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

    pub fn emit_filtered<E: SyncEvent>(
        &self,
        args: &E::Args,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) {
        for hook in self.event_snapshot::<E>(DispatchMode::Emit, args, filter) {
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

    pub fn bail_filtered<E: SyncEvent>(
        &self,
        args: &E::Args,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) -> Option<E::Output> {
        for hook in self.event_snapshot::<E>(DispatchMode::Bail, args, filter) {
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
    pub fn parallel<E: AsyncEvent>(&self, args: E::Args) -> Task<Result> {
        self.parallel_filtered::<E>(args, None)
    }

    pub fn parallel_filtered<E: AsyncEvent>(
        &self,
        args: E::Args,
        filter: Option<&dyn Fn(&Context) -> bool>,
    ) -> Task<Result> {
        let hooks = self.event_snapshot::<E>(DispatchMode::Parallel, &args, filter);
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
                    E::NAME
                ));
                error.causes = causes;
                Err(error)
            }
        })
    }

    /// S-EVT-7：逐个等待，首个 `Some` 结束派发。
    pub fn serial<E: AsyncEvent>(&self, args: E::Args) -> Task<Result<Option<E::Output>>> {
        self.serial_filtered::<E>(args, None)
    }

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
    pub fn waterfall<E: WaterfallEvent>(
        &self,
        args: E::Args,
        inner: impl Fn() -> Result<E::Output> + Send + Sync + 'static,
    ) -> Result<E::Output> {
        self.waterfall_filtered::<E>(args, inner, None)
    }

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

    pub fn listener_count<E: Event>(&self) -> usize {
        self.kernel
            .state
            .lock()
            .hooks
            .get(&TypeId::of::<E>())
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
