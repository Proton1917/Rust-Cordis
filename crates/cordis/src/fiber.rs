//! 纤程状态与纪元调和。

use crate::{
    EffectMeta, Result, Task,
    effect::Effect,
    kernel::{EffectId, FiberId, Kernel},
    list::DisposableList,
};
use futures_util::future::join_all;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiberState {
    Pending,
    Loading,
    Active,
    Failed,
    Disposed,
    Unloading,
}

pub(crate) struct FiberData {
    pub state: FiberState,
    pub effects: DisposableList<EffectId>,
}

impl FiberData {
    pub fn root() -> Self {
        Self {
            state: FiberState::Active,
            effects: DisposableList::default(),
        }
    }
}

#[derive(Clone)]
pub struct Fiber {
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) id: FiberId,
}

impl Fiber {
    pub fn name(&self) -> String {
        "root".into()
    }
    pub fn state(&self) -> FiberState {
        self.kernel.state.lock().fibers[self.id].state
    }

    pub fn effects(&self) -> Vec<EffectMeta> {
        let state = self.kernel.state.lock();
        state.fibers[self.id]
            .effects
            .iter()
            .map(|id| state.effects[*id].meta(&state))
            .collect()
    }

    /// S-FBR-17：根纤程释放效应后回到 Active。
    pub fn dispose(&self) -> Task<Result> {
        let effects = {
            let mut state = self.kernel.state.lock();
            state.fibers[self.id].state = FiberState::Unloading;
            let ids = state.fibers[self.id].effects.take();
            ids.into_iter()
                .rev()
                .map(|id| Effect::handle(&self.kernel, id, &mut state))
                .collect::<Vec<_>>()
        };
        let tasks: Vec<_> = effects.iter().map(Effect::dispose).collect();
        let fiber = self.clone();
        Box::pin(async move {
            let results = join_all(tasks).await;
            fiber.kernel.state.lock().fibers[fiber.id].state = FiberState::Active;
            results.into_iter().collect()
        })
    }
}
