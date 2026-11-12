//! 内核状态、调和队列与宿主接口。

use crate::{
    Error, Realm,
    context::Scope,
    effect::EffectData,
    events::{EventId, Hook},
    fiber::FiberData,
    logger::LoggerState,
    reflect::ImplData,
    registry::{ComponentDef, ComponentId, RuntimeData},
};
use parking_lot::Mutex;
use slotmap::{SlotMap, new_key_type};
use std::{
    any::TypeId,
    collections::{HashMap, VecDeque},
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// 可交给宿主执行器的拥有型异步任务，完成时产生 `T`。
pub type Task<T = ()> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// 宿主负责持续轮询已提交的任务。
pub trait Spawn: Send + Sync + 'static {
    /// 接管 `task`，调度并持续轮询至完成。
    /// 宿主派发失败的 panic 由内核转换为调度错误。
    fn spawn(&self, task: Task);
}

/// 宿主时间来源，供日志时间戳与异步等待使用。
pub trait Clock: Send + Sync + 'static {
    /// 返回相对于宿主时间基准的持续时间。
    fn now(&self) -> Duration;
    /// 创建表示指定等待时长的任务。
    fn sleep(&self, duration: Duration) -> Task;
}

new_key_type! {
    pub(crate) struct FiberId;
    pub(crate) struct EffectId;
    pub(crate) struct ImplId;
    struct SubmissionId;
}

pub(crate) struct Kernel {
    pub state: Mutex<State>,
    pub spawner: Arc<dyn Spawn>,
    pub clock: Arc<dyn Clock>,
}

pub(crate) struct State {
    pub fibers: SlotMap<FiberId, FiberData>,
    pub effects: SlotMap<EffectId, EffectData>,
    pub hooks: HashMap<EventId, Vec<Arc<Hook>>>,
    pub event_names: HashMap<Arc<str>, bool>,
    pub dirty: VecDeque<Arc<Scope>>,
    pub draining: bool,
    #[cfg(test)]
    pub panic_on_drain: bool,
    pub runtimes: HashMap<ComponentId, RuntimeData>,
    pub logger: LoggerState,
    pub aliases: HashMap<ComponentId, ComponentId>,
    pub definitions: HashMap<ComponentId, ComponentDef>,
    pub impls: SlotMap<ImplId, ImplData>,
    pub store: HashMap<(Arc<str>, Realm), ImplId>,
    pub names: HashMap<Arc<str>, Option<TypeId>>,
    pub impl_counter: u64,
    submissions: SlotMap<SubmissionId, Task>,
}

pub(crate) struct SubmissionFailure {
    pub error: Error,
    pub rejected: Option<Task>,
}

impl Kernel {
    pub(crate) fn report(self: &Arc<Self>, fiber: FiberId, error: Error) {
        crate::logger::record_error(self, fiber, error);
    }

    /// A-3、A-7：宿主接收调度句柄，任务在首次轮询时移出 State。
    /// 拒收时将任务所有权交回调用方；接收后发生的 panic 保留既有执行。
    pub(crate) fn submit(
        self: &Arc<Self>,
        task: Task,
    ) -> std::result::Result<(), SubmissionFailure> {
        let id = self.state.lock().submissions.insert(task);
        let started = Arc::new(AtomicBool::new(false));
        let lease = Arc::new(());
        let task_started = started.clone();
        let task_lease = lease.clone();
        let kernel = self.clone();
        let submission = Box::pin(async move {
            let _lease = task_lease;
            task_started.store(true, Ordering::Release);
            let task = kernel
                .state
                .lock()
                .submissions
                .remove(id)
                .expect("submitted task retained");
            task.await;
        });
        catch_unwind(AssertUnwindSafe(|| self.spawner.spawn(submission))).map_err(|payload| {
            let rejected = if started.load(Ordering::Acquire) || Arc::strong_count(&lease) > 1 {
                None
            } else {
                self.state.lock().submissions.remove(id)
            };
            SubmissionFailure {
                error: Error::panic(payload),
                rejected,
            }
        })
    }

    pub fn new<H: Spawn + Clock>(host: Arc<H>) -> (Arc<Self>, FiberId) {
        let mut fibers = SlotMap::with_key();
        let root = fibers.insert(FiberData::root());
        (
            Arc::new(Self {
                state: Mutex::new(State {
                    fibers,
                    effects: SlotMap::with_key(),
                    hooks: HashMap::new(),
                    event_names: HashMap::new(),
                    dirty: VecDeque::new(),
                    draining: false,
                    #[cfg(test)]
                    panic_on_drain: false,
                    runtimes: HashMap::new(),
                    logger: LoggerState::default(),
                    aliases: HashMap::new(),
                    definitions: HashMap::new(),
                    impls: SlotMap::with_key(),
                    store: HashMap::new(),
                    names: HashMap::new(),
                    impl_counter: 0,
                    submissions: SlotMap::with_key(),
                }),
                spawner: host.clone(),
                clock: host,
            }),
            root,
        )
    }
}
