//! 内核状态、调和队列与宿主接口。

use crate::{effect::EffectData, fiber::FiberData};
use parking_lot::Mutex;
use slotmap::{SlotMap, new_key_type};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

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
}

pub(crate) struct Kernel {
    pub state: Mutex<State>,
    pub spawner: Arc<dyn Spawn>,
    pub clock: Arc<dyn Clock>,
}

pub(crate) struct State {
    pub fibers: SlotMap<FiberId, FiberData>,
    pub effects: SlotMap<EffectId, EffectData>,
}

impl Kernel {
    pub fn new<H: Spawn + Clock>(host: Arc<H>) -> (Arc<Self>, FiberId) {
        let mut fibers = SlotMap::with_key();
        let root = fibers.insert(FiberData::root());
        (
            Arc::new(Self {
                state: Mutex::new(State {
                    fibers,
                    effects: SlotMap::with_key(),
                }),
                spawner: host.clone(),
                clock: host,
            }),
            root,
        )
    }
}
