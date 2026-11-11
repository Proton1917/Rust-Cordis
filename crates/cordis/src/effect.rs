//! 可回退效应与显式释放。

use crate::{
    Context, Error, Result, Task,
    kernel::{EffectId, FiberId, Kernel, State},
};
use futures_util::future::poll_fn;
use std::{
    panic::{AssertUnwindSafe, Location, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Poll, Waker},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectMeta {
    pub label: String,
    pub children: Vec<EffectMeta>,
}

pub(crate) enum Cleanup {
    Sync(Box<dyn FnOnce() -> Result + Send + Sync>),
    Child(Effect),
}

pub(crate) struct EffectData {
    pub owner: FiberId,
    pub refs: usize,
    pub armed: Arc<AtomicBool>,
    pub running: bool,
    pub cleaning: bool,
    pub steps: Vec<Cleanup>,
    pub label: String,
    pub children: Vec<EffectId>,
    pub parent: Option<EffectId>,
    pub outcome: Option<Result>,
    pub waiters: Vec<Waker>,
    pub location: &'static Location<'static>,
}

impl EffectData {
    pub fn meta(&self, state: &State) -> EffectMeta {
        EffectMeta {
            label: self.label.clone(),
            children: self
                .children
                .iter()
                .filter_map(|id| state.effects.get(*id))
                .map(|data| data.meta(state))
                .collect(),
        }
    }
}

/// A-8：句柄控制显式释放，所属纤程保留效应的所有权。
pub struct Effect {
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) id: EffectId,
    armed: Arc<AtomicBool>,
}

impl Clone for Effect {
    fn clone(&self) -> Self {
        Self::handle(&self.kernel, self.id, &mut self.kernel.state.lock())
    }
}

impl Drop for Effect {
    fn drop(&mut self) {
        let removed = {
            let mut state = self.kernel.state.lock();
            let data = &mut state.effects[self.id];
            data.refs -= 1;
            if data.refs == 0 && data.outcome.is_some() {
                state.effects.remove(self.id)
            } else {
                None
            }
        };
        drop(removed);
    }
}

#[derive(Clone)]
pub struct EffectScope(Effect);

impl EffectScope {
    pub(crate) fn effect_id(&self) -> EffectId {
        self.0.id
    }

    /// T-EFF-3：登记本步骤交回的逆变换。
    pub fn defer<F>(&self, cleanup: F) -> Result
    where
        F: FnOnce() -> Result + Send + Sync + 'static,
    {
        let mut state = self.0.kernel.state.lock();
        let data = &mut state.effects[self.0.id];
        if !data.running {
            return Err(Error::inactive());
        }
        data.steps.push(Cleanup::Sync(Box::new(cleanup)));
        Ok(())
    }

    /// S-EFF-6、T-EFF-6：将同一纤程的子效应移入当前效应。
    pub fn adopt(&self, child: Effect) -> Result {
        if !Arc::ptr_eq(&self.0.kernel, &child.kernel) {
            return Err(Error::new("effect belongs to another kernel"));
        }
        let mut state = self.0.kernel.state.lock();
        let parent = &state.effects[self.0.id];
        let child_data = &state.effects[child.id];
        if !parent.running || parent.owner != child_data.owner || child_data.parent.is_some() {
            return Err(Error::new("effect ownership is incompatible"));
        }
        let mut ancestor = Some(self.0.id);
        while let Some(id) = ancestor {
            if id == child.id {
                return Err(Error::new("effect adoption creates a cycle"));
            }
            ancestor = state.effects[id].parent;
        }
        let owner = child_data.owner;
        state.fibers[owner].effects.remove(&child.id);
        state.effects[child.id].parent = Some(self.0.id);
        let parent = &mut state.effects[self.0.id];
        parent.children.push(child.id);
        parent.steps.push(Cleanup::Child(child));
        Ok(())
    }

    pub fn is_current(&self) -> bool {
        self.0.armed.load(Ordering::Acquire)
    }
}

impl Context {
    /// T-EFF-1、S-EFF-1、S-EFF-2：同步执行效应体并登记逆变换。
    #[track_caller]
    pub fn effect<F>(&self, label: impl Into<String>, callback: F) -> Result<Effect>
    where
        F: FnOnce(EffectScope) -> Result + Send + Sync + 'static,
    {
        let location = Location::caller();
        let effect = self.create_effect(label.into(), location)?;
        let result = catch_unwind(AssertUnwindSafe(|| callback(EffectScope(effect.clone()))))
            .unwrap_or_else(|payload| Err(Error::panic(payload)))
            .map_err(|error| error.at(location));
        effect.finish_run(result.is_err());
        result.map(|()| effect)
    }

    fn create_effect(&self, label: String, location: &'static Location<'static>) -> Result<Effect> {
        let mut state = self.kernel.state.lock();
        let owner = self.scope.fiber;
        if state.fibers[owner].state == crate::FiberState::Disposed {
            return Err(Error::inactive().at(location));
        }
        let armed = Arc::new(AtomicBool::new(true));
        let id = state.effects.insert(EffectData {
            owner,
            refs: 1,
            armed: armed.clone(),
            running: true,
            cleaning: false,
            steps: Vec::new(),
            label,
            children: Vec::new(),
            parent: None,
            outcome: None,
            waiters: Vec::new(),
            location,
        });
        state.fibers[owner].effects.insert(id, false);
        Ok(Effect {
            kernel: self.kernel.clone(),
            id,
            armed,
        })
    }
}

impl Effect {
    pub(crate) fn handle(kernel: &Arc<Kernel>, id: EffectId, state: &mut State) -> Self {
        let data = &mut state.effects[id];
        data.refs += 1;
        Self {
            kernel: kernel.clone(),
            id,
            armed: data.armed.clone(),
        }
    }

    fn finish_run(&self, failed: bool) {
        {
            let mut state = self.kernel.state.lock();
            let data = &mut state.effects[self.id];
            data.running = false;
            if failed {
                data.armed.store(false, Ordering::Release);
            }
            let owner = data.owner;
            if data.parent.is_none() && !data.cleaning {
                state.fibers[owner].effects.remove(&self.id);
                state.fibers[owner].effects.insert(self.id, false);
            }
        }
        if !self.armed.load(Ordering::Acquire) {
            self.start_cleanup();
        }
    }

    /// T-EFF-2、S-EFF-4：同步使守卫失效，释放进度由宿主驱动。
    pub fn dispose(&self) -> Task<Result> {
        self.armed.store(false, Ordering::Release);
        self.start_cleanup();
        let effect = self.clone();
        Box::pin(poll_fn(move |cx| {
            let mut state = effect.kernel.state.lock();
            let data = &mut state.effects[effect.id];
            if let Some(result) = &data.outcome {
                return Poll::Ready(result.clone());
            }
            if !data.waiters.iter().any(|w| w.will_wake(cx.waker())) {
                data.waiters.push(cx.waker().clone());
            }
            Poll::Pending
        }))
    }

    fn start_cleanup(&self) {
        let steps = {
            let mut state = self.kernel.state.lock();
            let data = &mut state.effects[self.id];
            if data.running || data.cleaning {
                return;
            }
            data.cleaning = true;
            let steps = std::mem::take(&mut data.steps);
            let owner = data.owner;
            state.fibers[owner].effects.remove(&self.id);
            steps
        };
        self.clean_steps(steps, Ok(()));
    }

    /// S-EFF-3：单个效应的逆变换按相反顺序逐个完成。
    fn clean_steps(&self, mut steps: Vec<Cleanup>, mut result: Result) {
        while let Some(step) = steps.pop() {
            match step {
                Cleanup::Sync(callback) => {
                    let next = catch_unwind(AssertUnwindSafe(callback))
                        .unwrap_or_else(|p| Err(Error::panic(p)));
                    if result.is_ok() {
                        result = next;
                    }
                }
                Cleanup::Child(child) => {
                    let task = child.dispose();
                    let done = child.kernel.state.lock().effects[child.id].outcome.clone();
                    if let Some(next) = done {
                        if result.is_ok() {
                            result = next;
                        }
                    } else {
                        let effect = self.clone();
                        self.kernel.spawner.spawn(Box::pin(async move {
                            let next = task.await;
                            if result.is_ok() {
                                result = next;
                            }
                            effect.clean_steps(steps, result);
                        }));
                        return;
                    }
                }
            }
        }
        let waiters = {
            let mut state = self.kernel.state.lock();
            let data = &mut state.effects[self.id];
            data.outcome = Some(result.map_err(|e| e.at(data.location)));
            data.children.clear();
            std::mem::take(&mut data.waiters)
        };
        for waiter in waiters {
            waiter.wake();
        }
    }
}
