use cordis::{Context, EffectMeta, Error, Event, SyncKind};
use cordis_testkit::TestExecutor;
use std::sync::{Arc, Mutex};

struct Subscription;
impl Event for Subscription {
    const NAME: &'static str = "custom-event";
    type Args = ();
    type Output = ();
    type Kind = SyncKind;
}

fn meta(label: &str, children: Vec<EffectMeta>) -> EffectMeta {
    EffectMeta {
        label: label.into(),
        children,
    }
}

// DSP-02；T-EFF-1、T-EFF-2：释放资源并保持局部逆变换幂等。
#[test]
fn dispose_manually() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let released = Arc::new(Mutex::new(Vec::new()));
    let output = released.clone();
    let effect = root
        .effect("anonymous", move |fx| {
            fx.defer(move || {
                output.lock().unwrap().push("connection");
                Ok(())
            })
        })
        .unwrap();
    assert_eq!(root.fiber().effects(), [meta("anonymous", vec![])]);
    assert!(released.lock().unwrap().is_empty());
    host.finish(effect.dispose()).unwrap();
    assert_eq!(*released.lock().unwrap(), ["connection"]);
    host.finish(effect.dispose()).unwrap();
    assert_eq!(*released.lock().unwrap(), ["connection"]);
    assert!(root.fiber().effects().is_empty());
}

// DSP-03；T-EFF-3、T-EFF-6：显式 adopt 对应生成器交回子效应。
#[test]
fn yield_dispose() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let ctx = root.clone();
    let seq = values.clone();
    let effect = root
        .effect("anonymous", move |fx| {
            let a = seq.clone();
            fx.defer(move || {
                a.lock().unwrap().push(1);
                Ok(())
            })?;
            fx.adopt(ctx.on::<Subscription>(|_| None)?)?;
            let b = seq.clone();
            fx.defer(move || {
                b.lock().unwrap().push(2);
                Ok(())
            })?;
            let inner = ctx.clone();
            fx.adopt(ctx.effect("anonymous", move |child| {
                child.adopt(inner.on::<Subscription>(|_| None)?)?;
                child.defer(move || {
                    seq.lock().unwrap().push(3);
                    Ok(())
                })
            })?)
        })
        .unwrap();
    root.on::<Subscription>(|_| None).unwrap();
    assert_eq!(
        root.fiber().effects(),
        [
            meta(
                "anonymous",
                vec![
                    meta("ctx.on(\"custom-event\")", vec![]),
                    meta("anonymous", vec![meta("ctx.on(\"custom-event\")", vec![])])
                ]
            ),
            meta("ctx.on(\"custom-event\")", vec![])
        ]
    );
    assert!(values.lock().unwrap().is_empty());
    assert_eq!(root.listener_count::<Subscription>(), 3);
    host.finish(effect.dispose()).unwrap();
    assert_eq!(*values.lock().unwrap(), [3, 2, 1]);
    assert_eq!(root.listener_count::<Subscription>(), 1);
    host.finish(effect.dispose()).unwrap();
    assert_eq!(*values.lock().unwrap(), [3, 2, 1]);
    host.finish(root.fiber().dispose()).unwrap();
    assert!(root.fiber().effects().is_empty());
    assert_eq!(root.listener_count::<Subscription>(), 0);
}

// DSP-11：失败经 Result 返回，保持登记位置和原状态。
#[test]
fn return_with_error() {
    let root = Context::new(TestExecutor::new());
    let result = root.effect("failure", |_| Err(Error::new("test")));
    let error = result.err().unwrap();
    assert_eq!(&*error.message, "test");
    assert_eq!(error.locations[0].file(), file!());
    assert!(root.fiber().effects().is_empty());
}

// DSP-12：执行错误立即恢复此前已登记的步骤。
#[test]
fn yield_with_error() {
    let root = Context::new(TestExecutor::new());
    let values = Arc::new(Mutex::new(Vec::new()));
    let output = values.clone();
    let result = root.effect("failure", move |fx| {
        fx.defer(move || {
            output.lock().unwrap().push(1);
            Ok(())
        })?;
        Err(Error::new("test"))
    });
    assert_eq!(&*result.err().unwrap().message, "test");
    assert_eq!(*values.lock().unwrap(), [1]);
    assert!(root.fiber().effects().is_empty());
}

// T-EFF-7、A-1、A-2、A-3、A-8：外部资源由用户逆变换恢复；跨线程清理可重入同一内核。
#[test]
fn reentrant_cleanup_and_root_ownership() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let resources = Arc::new(Mutex::new(vec![1, 2]));
    let ctx = root.clone();
    let values = resources.clone();
    root.effect("outer", move |fx| {
        fx.defer(move || {
            values.lock().unwrap().pop();
            let remaining = values.clone();
            let inner = ctx.effect("inner", move |fx| {
                fx.defer(move || {
                    remaining.lock().unwrap().pop();
                    Ok(())
                })
            })?;
            drop(inner.dispose());
            Ok(())
        })
    })
    .unwrap();
    assert_eq!(*resources.lock().unwrap(), [1, 2]);
    let fiber = root.fiber();
    std::thread::spawn(move || {
        host.finish(fiber.dispose()).unwrap();
    })
    .join()
    .unwrap();
    assert!(resources.lock().unwrap().is_empty());
    assert_eq!(root.fiber().state(), cordis::FiberState::Active);
    assert!(root.fiber().effects().is_empty());
}

// A-9：已登记资源在效应体 panic 后恢复，后续内核操作继续成功。
#[test]
fn panic_recovers_registered_steps() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(vec![7]));
    let output = values.clone();
    let result = root.effect("panic", move |fx| {
        fx.defer(move || {
            output.lock().unwrap().clear();
            Ok(())
        })?;
        panic!("effect panic");
    });
    assert_eq!(&*result.err().unwrap().message, "effect panic");
    assert!(values.lock().unwrap().is_empty());
    let next = root.effect("next", |_| Ok(())).unwrap();
    host.finish(next.dispose()).unwrap();
}
