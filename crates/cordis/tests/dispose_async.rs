use cordis::{Clock, Context, Effect, Error, Result};
use cordis_testkit::TestExecutor;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

type Sequence = Arc<Mutex<Vec<u32>>>;

// 时间推进遵循每个在途步骤的实际截止时刻。

fn setup() -> (Arc<TestExecutor>, Context, Sequence) {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    (host, root, Arc::new(Mutex::new(Vec::new())))
}

fn advance(host: &TestExecutor, millis: u64) {
    host.advance(Duration::from_millis(millis));
    host.run_until_stalled();
}

fn sequence(root: &Context, host: Arc<TestExecutor>, values: Sequence) -> Effect {
    root.effect_async("sequence", move |fx| async move {
        for value in [1, 3, 5] {
            if !fx.is_current() {
                break;
            }
            host.sleep(Duration::from_millis(100)).await;
            values.lock().unwrap().push(value);
            let output = values.clone();
            fx.defer(move || {
                output.lock().unwrap().push(value + 1);
                Ok(())
            })?;
        }
        Ok(())
    })
    .unwrap()
}

// DSP-04、DSP-05；T-EFF-5、T-LIF-6：单步 Future 结束后交回逆变换。
#[test]
fn async_return() {
    for early in [false, true] {
        let (host, root, values) = setup();
        let clock = host.clone();
        let output = values.clone();
        let effect = root
            .effect_async("single", move |fx| async move {
                clock.sleep(Duration::from_millis(100)).await;
                output.lock().unwrap().push(1);
                fx.defer(move || {
                    output.lock().unwrap().push(2);
                    Ok(())
                })
            })
            .unwrap();
        host.run_until_stalled();
        if early {
            drop(effect.dispose());
        }
        assert!(values.lock().unwrap().is_empty());
        advance(&host, 100);
        if early {
            assert_eq!(*values.lock().unwrap(), [1, 2]);
        } else {
            assert_eq!(*values.lock().unwrap(), [1]);
        }
        host.finish(effect.dispose()).unwrap();
        assert_eq!(*values.lock().unwrap(), [1, 2]);
    }
}

// DSP-06、DSP-10；T-EFF-3：异步步骤顺序执行，逆变换反向执行。
#[test]
fn async_yield_and_wait_for_execution() {
    for wait in [false, true] {
        let (host, root, values) = setup();
        let effect = sequence(&root, host.clone(), values.clone());
        host.run_until_stalled();
        assert!(values.lock().unwrap().is_empty());
        for _ in 0..3 {
            advance(&host, 100);
        }
        assert_eq!(*values.lock().unwrap(), [1, 3, 5]);
        let disposer = if wait {
            host.finish(effect.settled()).unwrap()
        } else {
            effect
        };
        host.finish(disposer.dispose()).unwrap();
        assert_eq!(*values.lock().unwrap(), [1, 3, 5, 6, 4, 2]);
    }
}

// DSP-07、DSP-08；T-EFF-4：保留在途步骤的最后一次交回。
#[test]
fn async_yield_cooperative_stop() {
    for after_first in [false, true] {
        let (host, root, values) = setup();
        let effect = sequence(&root, host.clone(), values.clone());
        host.run_until_stalled();
        advance(&host, if after_first { 100 } else { 50 });
        let disposing = effect.dispose();
        if after_first {
            assert_eq!(*values.lock().unwrap(), [1]);
        } else {
            assert!(values.lock().unwrap().is_empty());
        }
        advance(&host, 300);
        host.finish(disposing).unwrap();
        assert_eq!(
            *values.lock().unwrap(),
            if after_first {
                vec![1, 3, 4, 2]
            } else {
                vec![1, 2]
            }
        );
    }
}

// DSP-09：协作退出路径执行局部收束逻辑，然后恢复已交回的步骤。
#[test]
fn async_yield_finalization() {
    let (host, root, values) = setup();
    let clock = host.clone();
    let output = values.clone();
    let effect = root
        .effect_async("finalization", move |fx| async move {
            let body: Result = async {
                for value in 1..=3 {
                    if !fx.is_current() {
                        break;
                    }
                    if value > 1 {
                        clock.sleep(Duration::from_millis(100)).await;
                    }
                    let output = output.clone();
                    fx.defer(move || {
                        output.lock().unwrap().push(value);
                        Ok(())
                    })?;
                }
                Ok(())
            }
            .await;
            output.lock().unwrap().push(0);
            body
        })
        .unwrap();
    host.run_until_stalled();
    advance(&host, 50);
    let disposing = effect.dispose();
    advance(&host, 100);
    host.finish(disposing).unwrap();
    assert_eq!(*values.lock().unwrap(), [0, 2, 1]);
}

// DSP-13、DSP-14：异步错误回退部分效应，并交给等待者。
#[test]
fn async_error_recovers_registered_steps() {
    for partial in [false, true] {
        let (host, root, values) = setup();
        let output = values.clone();
        let effect = root
            .effect_async("failure", move |fx| async move {
                if partial {
                    fx.defer(move || {
                        output.lock().unwrap().push(1);
                        Ok(())
                    })?;
                }
                Err(Error::new("test"))
            })
            .unwrap();
        assert!(values.lock().unwrap().is_empty());
        assert_eq!(
            &*host.finish(effect.settled()).err().unwrap().message,
            "test"
        );
        assert_eq!(
            *values.lock().unwrap(),
            if partial { vec![1] } else { vec![] }
        );
        assert!(root.fiber().effects().is_empty());
        assert_eq!(&*root.errors()[0].message, "test");
    }
}

// S-EFF-3、A-9：异步释放完整结束后继续前序释放，错误保留。
#[test]
fn async_cleanup_is_sequential_and_panic_safe() {
    let (host, root, values) = setup();
    let clock = host.clone();
    let output = values.clone();
    let effect = root
        .effect("cleanup", move |fx| {
            let first = output.clone();
            fx.defer(move || {
                first.lock().unwrap().push(1);
                Ok(())
            })?;
            fx.defer_async(move || async move {
                clock.sleep(Duration::from_millis(10)).await;
                output.lock().unwrap().push(2);
                panic!("cleanup panic");
            })
        })
        .unwrap();
    let disposing = effect.dispose();
    host.run_until_stalled();
    assert!(values.lock().unwrap().is_empty());
    advance(&host, 10);
    assert_eq!(
        &*host.finish(disposing).unwrap_err().message,
        "cleanup panic"
    );
    assert_eq!(*values.lock().unwrap(), [2, 1]);
}
