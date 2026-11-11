//! 内核状态、调和队列与宿主接口。

use crate::{
    Error, Realm,
    effect::EffectData,
    events::Hook,
    fiber::FiberData,
    reflect::ImplData,
    registry::{ComponentId, RuntimeData},
};
use parking_lot::Mutex;
use slotmap::{SlotMap, new_key_type};
use std::{
    any::TypeId,
    collections::{HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

pub type Task<T = ()> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// 宿主负责持续轮询已提交的任务。
pub trait Spawn: Send + Sync + 'static {
    fn spawn(&self, task: Task);
}

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration) -> Task;
}

new_key_type! {
    pub(crate) struct FiberId;
    pub(crate) struct EffectId;
    pub(crate) struct ImplId;
}

pub(crate) struct Kernel {
    pub state: Mutex<State>,
    pub spawner: Arc<dyn Spawn>,
    pub clock: Arc<dyn Clock>,
}

pub(crate) struct State {
    pub fibers: SlotMap<FiberId, FiberData>,
    pub effects: SlotMap<EffectId, EffectData>,
    pub hooks: HashMap<TypeId, Vec<Arc<Hook>>>,
    pub dirty: VecDeque<FiberId>,
    pub draining: bool,
    pub runtimes: HashMap<ComponentId, RuntimeData>,
    pub errors: Vec<(FiberId, Error)>,
    pub impls: SlotMap<ImplId, ImplData>,
    pub store: HashMap<(TypeId, Realm), ImplId>,
    pub names: HashMap<&'static str, TypeId>,
    pub impl_counter: u64,
}

impl Kernel {
    pub(crate) fn report(&self, fiber: FiberId, error: Error) {
        self.state.lock().errors.push((fiber, error));
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
                    dirty: VecDeque::new(),
                    draining: false,
                    runtimes: HashMap::new(),
                    errors: Vec::new(),
                    impls: SlotMap::with_key(),
                    store: HashMap::new(),
                    names: HashMap::new(),
                    impl_counter: 0,
                }),
                spawner: host.clone(),
                clock: host,
            }),
            root,
        )
    }
}
