//! 确定性任务调度与可推进时钟。

pub mod build;
pub mod cleanup;

use cordis::{Clock, Spawn, Task};
use futures_util::{
    future::poll_fn,
    task::{ArcWake, waker_ref},
};
use std::{
    collections::VecDeque,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

#[derive(Default)]
/// 在调用线程按队列顺序推进任务，并由调用方显式推进时间。
pub struct TestExecutor {
    queue: Arc<Mutex<VecDeque<Arc<Scheduled>>>>,
    tasks: Mutex<Vec<Arc<Scheduled>>>,
    time: Arc<Mutex<Time>>,
}

#[derive(Default)]
struct Time {
    now: Duration,
    sleepers: Vec<(Duration, Waker)>,
}

struct Scheduled {
    task: Mutex<Option<Task>>,
    queue: Arc<Mutex<VecDeque<Arc<Scheduled>>>>,
    queued: AtomicBool,
}

impl ArcWake for Scheduled {
    fn wake_by_ref(task: &Arc<Self>) {
        if !task.queued.swap(true, Ordering::AcqRel) {
            task.queue.lock().unwrap().push_back(task.clone());
        }
    }
}

impl Spawn for TestExecutor {
    fn spawn(&self, task: Task) {
        let task = Arc::new(Scheduled {
            task: Mutex::new(Some(task)),
            queue: self.queue.clone(),
            queued: AtomicBool::new(true),
        });
        self.tasks.lock().unwrap().push(task.clone());
        self.queue.lock().unwrap().push_back(task);
    }
}

impl Clock for TestExecutor {
    fn now(&self) -> Duration {
        self.time.lock().unwrap().now
    }

    fn sleep(&self, duration: Duration) -> Task {
        let deadline = self.now() + duration;
        let time = self.time.clone();
        Box::pin(poll_fn(move |cx| {
            let mut time = time.lock().unwrap();
            if time.now >= deadline {
                return Poll::Ready(());
            }
            if !time
                .sleepers
                .iter()
                .any(|(at, w)| *at == deadline && w.will_wake(cx.waker()))
            {
                time.sleepers.push((deadline, cx.waker().clone()));
            }
            Poll::Pending
        }))
    }
}

impl TestExecutor {
    /// 创建共享执行器，供 Context 接收 Spawn 与 Clock 能力。
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 轮询当前可运行任务，直到全部完成或等待外部唤醒。
    pub fn run_until_stalled(&self) {
        loop {
            let next = self.queue.lock().unwrap().pop_front();
            let Some(task) = next else {
                return;
            };
            task.queued.store(false, Ordering::Release);
            let future = task.task.lock().unwrap().take();
            if let Some(mut future) = future {
                let waker = waker_ref(&task);
                if future
                    .as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
                {
                    *task.task.lock().unwrap() = Some(future);
                } else {
                    self.tasks
                        .lock()
                        .unwrap()
                        .retain(|pending| !Arc::ptr_eq(pending, &task));
                }
            }
        }
    }

    /// 推进逻辑时间并唤醒到期等待；后续轮询由 run_until_stalled 执行。
    pub fn advance(&self, duration: Duration) {
        let ready = {
            let mut time = self.time.lock().unwrap();
            time.now += duration;
            let sleepers = std::mem::take(&mut time.sleepers);
            let mut ready = Vec::new();
            for (deadline, waker) in sleepers {
                if deadline <= time.now {
                    ready.push(waker);
                } else {
                    time.sleepers.push((deadline, waker));
                }
            }
            ready
        };
        for waker in ready {
            waker.wake();
        }
    }

    /// 推进可运行任务，并要求传入操作已经具备完成条件。
    pub fn finish<F: Future>(&self, future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        self.run_until_stalled();
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => {
                panic!("operation is waiting; advance its clock or release its dependency")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_tasks_and_explicit_time() {
        let host = TestExecutor::new();
        let values = Arc::new(Mutex::new(Vec::new()));
        for i in 0..3 {
            let values = values.clone();
            let clock = host.clone();
            host.spawn(Box::pin(async move {
                values.lock().unwrap().push(i);
                clock.sleep(Duration::from_millis(10)).await;
                values.lock().unwrap().push(i + 3);
            }));
        }
        host.run_until_stalled();
        assert_eq!(*values.lock().unwrap(), [0, 1, 2]);
        host.advance(Duration::from_millis(9));
        host.run_until_stalled();
        assert_eq!(*values.lock().unwrap(), [0, 1, 2]);
        host.advance(Duration::from_millis(1));
        host.run_until_stalled();
        assert_eq!(*values.lock().unwrap(), [0, 1, 2, 3, 4, 5]);
    }
}
