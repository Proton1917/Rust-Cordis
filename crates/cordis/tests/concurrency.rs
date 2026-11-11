use cordis::{Clock, ComponentDef, Context, FiberState, Inject, InternalStatus, Spawn, Task};
use cordis_testkit::TestExecutor;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    task::{Context as TaskContext, Poll, Waker},
    thread,
    time::Duration,
};

struct FallibleHost {
    executor: Arc<TestExecutor>,
    reject: AtomicUsize,
    panic_after_accept: AtomicBool,
}

impl Spawn for FallibleHost {
    fn spawn(&self, task: Task) {
        if self
            .reject
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            panic!("host rejected submission");
        }
        self.executor.spawn(task);
        if self.panic_after_accept.swap(false, Ordering::SeqCst) {
            panic!("host panicked after acceptance");
        }
    }
}

impl Clock for FallibleHost {
    fn now(&self) -> Duration {
        self.executor.now()
    }

    fn sleep(&self, duration: Duration) -> Task {
        self.executor.sleep(duration)
    }
}

#[test]
fn rejected_spawn_records_failure_and_preserves_reconciliation() {
    let executor = TestExecutor::new();
    let host = Arc::new(FallibleHost {
        executor: executor.clone(),
        reject: AtomicUsize::new(1),
        panic_after_accept: AtomicBool::new(false),
    });
    let root = Context::new(host);
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        root.inject(Inject::new(), |_| async { Ok(()) })
    }));
    let healthy = root.inject(Inject::new(), |_| async { Ok(()) }).unwrap();
    executor.run_until_stalled();
    assert_eq!(healthy.state(), FiberState::Active);
    let rejected = rejected
        .expect("submission failure becomes a fiber error")
        .unwrap();
    assert!(rejected.is_settled());
    assert_eq!(rejected.state(), FiberState::Failed);
    assert_eq!(
        executor
            .finish(rejected.settled())
            .unwrap_err()
            .message
            .as_ref(),
        "host rejected submission"
    );
    assert_eq!(
        root.errors().last().unwrap().message.as_ref(),
        "host rejected submission"
    );
    executor.finish(root.fiber().dispose()).unwrap();
}

fn fallible_host() -> Arc<FallibleHost> {
    Arc::new(FallibleHost {
        executor: TestExecutor::new(),
        reject: AtomicUsize::new(0),
        panic_after_accept: AtomicBool::new(false),
    })
}

#[test]
fn accepted_submission_survives_host_panic_once() {
    let host = fallible_host();
    let root = Context::new(host.clone());
    let loaded = Arc::new(AtomicUsize::new(0));
    let count = loaded.clone();
    host.panic_after_accept.store(true, Ordering::SeqCst);
    let fiber = root
        .inject(Inject::new(), move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        })
        .unwrap();
    host.executor.finish(fiber.settled()).unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(loaded.load(Ordering::SeqCst), 1);
    assert_eq!(
        root.errors().last().unwrap().message.as_ref(),
        "host panicked after acceptance"
    );
    host.executor.finish(fiber.dispose()).unwrap();
}

#[test]
fn rejected_effect_execution_returns_recorded_error() {
    let host = fallible_host();
    let root = Context::new(host.clone());
    let loaded = Arc::new(AtomicUsize::new(0));
    let count = loaded.clone();
    host.reject.store(1, Ordering::SeqCst);
    let effect = root
        .effect_async("rejected", move |_| async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        host.executor
            .finish(effect.settled())
            .err()
            .unwrap()
            .message
            .as_ref(),
        "host rejected submission"
    );
    assert_eq!(loaded.load(Ordering::SeqCst), 0);
    host.executor.finish(effect.dispose()).unwrap();
    assert_eq!(root.errors().len(), 1);
}

#[test]
fn rejected_effect_cleanup_resumes_original_future_on_dispose() {
    let host = fallible_host();
    let root = Context::new(host.clone());
    let started = Arc::new(AtomicUsize::new(0));
    let ended = Arc::new(AtomicUsize::new(0));
    let starts = started.clone();
    let ends = ended.clone();
    let clock = root.clone();
    let effect = root
        .effect("retained cleanup", move |fx| {
            fx.defer_async(move || {
                starts.fetch_add(1, Ordering::SeqCst);
                async move {
                    clock.sleep(Duration::from_millis(10)).await;
                    ends.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
        })
        .unwrap();
    host.reject.store(2, Ordering::SeqCst);
    assert_eq!(
        host.executor
            .finish(effect.dispose())
            .unwrap_err()
            .message
            .as_ref(),
        "host rejected submission"
    );
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(ended.load(Ordering::SeqCst), 0);
    let resumed = effect.dispose();
    host.executor.run_until_stalled();
    host.executor.advance(Duration::from_millis(10));
    host.executor.finish(resumed).unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(ended.load(Ordering::SeqCst), 1);
    assert_eq!(root.errors().len(), 2);
}

#[test]
fn rejected_fiber_cleanup_resumes_original_transition_on_dispose() {
    let host = fallible_host();
    let root = Context::new(host.clone());
    let released = Arc::new(AtomicUsize::new(0));
    let count = released.clone();
    let fiber = root
        .inject(Inject::new(), move |ctx| {
            let count = count.clone();
            async move {
                ctx.effect("resource", move |fx| {
                    fx.defer(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.executor.finish(fiber.settled()).unwrap();
    host.reject.store(2, Ordering::SeqCst);
    assert_eq!(
        host.executor
            .finish(fiber.dispose())
            .unwrap_err()
            .message
            .as_ref(),
        "host rejected submission"
    );
    assert_eq!(fiber.state(), FiberState::Failed);
    assert!(!fiber.is_settled());
    assert_eq!(released.load(Ordering::SeqCst), 0);
    host.executor.finish(fiber.dispose()).unwrap();
    assert_eq!(fiber.state(), FiberState::Disposed);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(root.errors().len(), 2);
    host.executor.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn restart_queued_behind_another_thread_must_not_settle_early() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(AtomicUsize::new(0));
    let count = applied.clone();
    let def_a = ComponentDef::new("a", Inject::new(), move |_, _: usize| {
        count.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    let def_b = ComponentDef::new("b", Inject::new(), |_, _: usize| async { Ok(()) });
    let a = root.mount_with(&def_a, 0usize).unwrap();
    let b = root.mount_with(&def_b, 0usize).unwrap();
    host.finish(a.settled()).unwrap();
    host.finish(b.settled()).unwrap();
    assert_eq!(applied.load(Ordering::SeqCst), 1);

    let gate = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let entered_tx = Mutex::new(entered_tx);
    let release_rx = Mutex::new(release_rx);
    let armed = gate.clone();
    root.on::<InternalStatus>(move |_| {
        if armed.swap(false, Ordering::SeqCst) {
            entered_tx.lock().unwrap().send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
        }
        None
    })
    .unwrap();

    // 线程 A：重启 b，在状态监听器里停住，此时它是唯一的调和者。
    gate.store(true, Ordering::SeqCst);
    let other = b.clone();
    let worker = thread::spawn(move || drop(other.restart()));
    entered_rx.recv().unwrap();

    // 主线程：重启 a。刷新请求排在 A 的队列里，尚未被处理。
    let mut task = a.restart();
    let early = task
        .as_mut()
        .poll(&mut TaskContext::from_waker(Waker::noop()));
    let settled_early = matches!(early, Poll::Ready(_));

    release_tx.send(()).unwrap();
    worker.join().unwrap();
    host.run_until_stalled();
    println!(
        "settled_early={settled_early} applied_at_return=1 applied_final={}",
        applied.load(Ordering::SeqCst)
    );
    assert!(
        !settled_early,
        "restart() 在重启发生之前就报告完成（当时 apply 次数仍为 1）"
    );
}
