//! ADR-0051：在调用取消时报告原生轮询占满，观测范围限调度事实。

use parking_lot::Mutex;
use std::{
    cell::RefCell,
    future::{Future, poll_fn},
    pin::pin,
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

thread_local! {
    static CURRENT: RefCell<Option<Arc<PollState>>> = const { RefCell::new(None) };
}

pub(super) struct Monitor {
    epoch: Instant,
    workers: usize,
    group: OnceLock<String>,
    states: Mutex<Vec<Weak<PollState>>>,
    reported: AtomicBool,
}

struct PollState {
    monitor: Arc<Monitor>,
    started: AtomicU64,
    method: Mutex<String>,
}

impl Monitor {
    pub(super) fn new(workers: usize) -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            workers,
            group: OnceLock::new(),
            states: Mutex::new(Vec::new()),
            reported: AtomicBool::new(false),
        })
    }

    pub(super) fn group(&self, group: String) {
        self.group.get_or_init(|| group);
    }

    fn tick(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64 + 1
    }

    /// 每个任务登记一次，poll 起止通过原子时间戳与线程局部引用记录。
    pub(super) async fn track<F: Future>(self: Arc<Self>, future: F) -> F::Output {
        let state = Arc::new(PollState {
            monitor: self.clone(),
            started: AtomicU64::new(0),
            method: Mutex::new("native task".into()),
        });
        {
            let mut states = self.states.lock();
            states.retain(|entry| entry.strong_count() > 0);
            states.push(Arc::downgrade(&state));
        }
        let mut future = pin!(future);
        poll_fn(|cx| {
            let _scope = PollScope::enter(state.clone());
            future.as_mut().poll(cx)
        })
        .await
    }

    /// ADR-0051：取消叠加持续占满时输出一次事实诊断，线程恢复后开启下一阶段。
    pub(super) fn cancelled(&self) {
        let now = self.tick();
        let active: Vec<_> = self
            .states
            .lock()
            .iter()
            .filter_map(Weak::upgrade)
            .filter_map(|state| {
                let started = state.started.load(Ordering::Acquire);
                (started > 0).then(|| (now.saturating_sub(started), state))
            })
            .collect();
        let duration = active
            .iter()
            .map(|(duration, _)| *duration)
            .min()
            .unwrap_or(0);
        if active.len() < self.workers
            || duration < 100
            || self.reported.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let methods: Vec<_> = active
            .iter()
            .map(|(_, state)| state.method.lock().clone())
            .collect();
        eprintln!(
            "原生执行线程持续占满：group={:?} methods={methods:?} occupied={}/{} duration_ms={duration}；排查阻塞等待（自行 block_on）或长时间同步计算",
            self.group
                .get()
                .map(String::as_str)
                .unwrap_or("initializing"),
            active.len(),
            self.workers,
        );
    }
}

pub(crate) fn method(name: &str) {
    CURRENT.with(|current| {
        if let Some(state) = current.borrow().as_ref() {
            *state.method.lock() = name.to_owned();
        }
    });
}

struct PollScope {
    state: Arc<PollState>,
    previous: Option<Arc<PollState>>,
}

impl PollScope {
    fn enter(state: Arc<PollState>) -> Self {
        state.started.store(state.monitor.tick(), Ordering::Release);
        let previous = CURRENT.with(|current| current.replace(Some(state.clone())));
        Self { state, previous }
    }
}

impl Drop for PollScope {
    fn drop(&mut self) {
        self.state.started.store(0, Ordering::Release);
        self.state.monitor.reported.store(false, Ordering::Release);
        CURRENT.with(|current| current.replace(self.previous.take()));
    }
}

/// 宿主协同等待期间，该原生轮询已交出工作线程；嵌套等待复用外层挂起。
pub(super) struct Suspension(Option<Arc<PollState>>);

impl Suspension {
    pub(super) fn enter() -> Self {
        Self(CURRENT.with(|current| {
            current.borrow().as_ref().and_then(|state| {
                (state.started.swap(0, Ordering::AcqRel) > 0).then(|| {
                    state.monitor.reported.store(false, Ordering::Release);
                    state.clone()
                })
            })
        }))
    }
}

impl Drop for Suspension {
    fn drop(&mut self) {
        if let Some(state) = &self.0 {
            state.started.store(state.monitor.tick(), Ordering::Release);
        }
    }
}
