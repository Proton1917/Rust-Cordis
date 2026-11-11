use cordis::{
    AsyncKind, Clock, Context, DispatchMode, Effect, Error, Event, EventOptions, InternalDispatch,
    InternalListener, Next, Result, Spawn, SyncKind, Task, WaterfallKind,
};
use cordis_testkit::TestExecutor;
use futures_util::future::join;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Number;
impl Event for Number {
    const NAME: &'static str = "custom-event";
    type Args = i32;
    type Output = i32;
    type Kind = SyncKind;
}

struct OtherNumber;
impl Event for OtherNumber {
    const NAME: &'static str = "custom-event";
    type Args = i32;
    type Output = i32;
    type Kind = SyncKind;
}

struct AsyncNumber;
impl Event for AsyncNumber {
    const NAME: &'static str = "custom-event";
    type Args = i32;
    type Output = i32;
    type Kind = AsyncKind;
}

struct Water;
impl Event for Water {
    const NAME: &'static str = "test/waterfall";
    type Args = i32;
    type Output = i32;
    type Kind = WaterfallKind;
}

struct AsyncWater;
impl Event for AsyncWater {
    const NAME: &'static str = "test/async-waterfall";
    type Args = i32;
    type Output = Task<Result<i32>>;
    type Kind = WaterfallKind;
}

type Values = Arc<Mutex<Vec<i32>>>;

fn setup() -> (Context, Arc<TestExecutor>, Values) {
    let host = TestExecutor::new();
    (
        Context::new(host.clone()),
        host,
        Arc::new(Mutex::new(Vec::new())),
    )
}

fn record(values: &Values, value: i32) -> Option<i32> {
    values.lock().unwrap().push(value);
    None
}

fn ready(value: Option<i32>) -> Task<Result<Option<i32>>> {
    Box::pin(async move { Ok(value) })
}

// EVT-03：类型身份对应 Symbol 身份，各派发族保留相同的可观察行为。
#[test]
fn typed_keys_across_dispatch_modes() {
    let (root, host, values) = setup();
    let trace = values.clone();
    let sync = root
        .on::<Number>(move |value| {
            record(&trace, *value);
            Some(*value)
        })
        .unwrap();
    root.on::<OtherNumber>(|_| panic!("distinct event type was dispatched"))
        .unwrap();
    let trace = values.clone();
    let asynchronous = root
        .on_async::<AsyncNumber>(
            move |value| {
                record(&trace, value);
                ready(Some(value))
            },
            EventOptions::default(),
        )
        .unwrap();
    root.emit::<Number>(&1);
    assert_eq!(root.bail::<Number>(&2), Some(2));
    assert_eq!(host.finish(root.serial::<AsyncNumber>(3)).unwrap(), Some(3));
    host.finish(root.parallel::<AsyncNumber>(4)).unwrap();
    assert_eq!(*values.lock().unwrap(), [1, 2, 3, 4]);
    host.finish(sync.dispose()).unwrap();
    host.finish(asynchronous.dispose()).unwrap();
    root.emit::<Number>(&5);
    host.finish(root.parallel::<AsyncNumber>(6)).unwrap();
    assert_eq!(*values.lock().unwrap(), [1, 2, 3, 4]);
}

// EVT-06、T-EFF-1、T-EFF-2：效应逐一移除监听器，最后一次释放恢复事件表。
#[test]
fn empty_buckets_are_removed() {
    let (root, host, _) = setup();
    let baseline = root.event_count();
    let first = root.on::<Number>(|_| None).unwrap();
    let second = root.on::<Number>(|_| None).unwrap();
    host.finish(first.dispose()).unwrap();
    assert_eq!(root.listener_count::<Number>(), 1);
    assert_eq!(root.event_count(), baseline + 1);
    host.finish(second.dispose()).unwrap();
    assert_eq!(root.event_count(), baseline);
    host.finish(second.dispose()).unwrap();
    assert_eq!(root.event_count(), baseline);
}

// EVT-07、S-EVT-1。
#[test]
fn on_and_explicit_disposal() {
    let (root, host, values) = setup();
    let trace = values.clone();
    let effect = root
        .on::<Number>(move |value| record(&trace, *value))
        .unwrap();
    root.emit::<Number>(&1);
    root.emit::<Number>(&2);
    assert_eq!(*values.lock().unwrap(), [1, 2]);
    host.finish(effect.dispose()).unwrap();
    root.emit::<Number>(&3);
    assert_eq!(*values.lock().unwrap(), [1, 2]);
}

// EVT-04、EVT-08、S-EVT-9：类型键的一次性登记及显式释放。
#[test]
fn once_and_explicit_disposal() {
    let (root, host, values) = setup();
    let trace = values.clone();
    let effect = root
        .once::<Number>(move |value| record(&trace, *value))
        .unwrap();
    root.emit::<Number>(&1);
    root.emit::<Number>(&2);
    host.finish(effect.dispose()).unwrap();
    root.emit::<Number>(&3);
    assert_eq!(*values.lock().unwrap(), [1]);
    assert_eq!(root.listener_count::<Number>(), 0);
}

// EVT-09、S-EVT-9、A-3。
#[test]
fn once_is_consumed_by_recursive_dispatch() {
    let (root, _, values) = setup();
    let nested = AtomicBool::new(false);
    let context = root.clone();
    root.on::<Number>(move |_| {
        if !nested.swap(true, Ordering::SeqCst) {
            context.emit::<Number>(&2);
        }
        None
    })
    .unwrap();
    let trace = values.clone();
    root.once::<Number>(move |value| record(&trace, *value))
        .unwrap();
    root.emit::<Number>(&1);
    assert_eq!(*values.lock().unwrap(), [2]);
}

// EVT-10：两个 serial 都停在前置监听器，恢复后共同争用一次性登记。
#[test]
fn once_across_concurrent_serial_dispatches() {
    let (root, host, values) = setup();
    let clock = host.clone();
    root.on_async::<AsyncNumber>(
        move |_| {
            let sleep = clock.sleep(Duration::from_millis(1));
            Box::pin(async move {
                sleep.await;
                Ok(None)
            })
        },
        EventOptions::default(),
    )
    .unwrap();
    let trace = values.clone();
    root.once_async::<AsyncNumber>(
        move |value| {
            record(&trace, value);
            ready(None)
        },
        EventOptions::default(),
    )
    .unwrap();
    let first = root.serial::<AsyncNumber>(1);
    let second = root.serial::<AsyncNumber>(2);
    let completed = Arc::new(AtomicBool::new(false));
    let done = completed.clone();
    host.spawn(Box::pin(async move {
        let (first, second) = join(first, second).await;
        assert_eq!(first.unwrap(), None);
        assert_eq!(second.unwrap(), None);
        done.store(true, Ordering::SeqCst);
    }));
    host.run_until_stalled();
    assert!(values.lock().unwrap().is_empty());
    host.advance(Duration::from_millis(1));
    host.run_until_stalled();
    assert!(completed.load(Ordering::SeqCst));
    assert_eq!(*values.lock().unwrap(), [1]);
}

// EVT-01、EVT-11：快照取得后，四种派发均检查释放状态。
#[test]
fn all_modes_skip_removed_listeners() {
    for mode in [
        DispatchMode::Emit,
        DispatchMode::Bail,
        DispatchMode::Parallel,
        DispatchMode::Serial,
    ] {
        let (root, host, values) = setup();
        let target: Arc<Mutex<Option<Effect>>> = Arc::new(Mutex::new(None));
        if matches!(mode, DispatchMode::Emit | DispatchMode::Bail) {
            let slot = target.clone();
            root.on::<Number>(move |_| {
                let effect = slot.lock().unwrap().take().unwrap();
                drop(effect.dispose());
                None
            })
            .unwrap();
            let trace = values.clone();
            *target.lock().unwrap() = Some(root.on::<Number>(move |_| record(&trace, 0)).unwrap());
            let trace = values.clone();
            root.on::<Number>(move |_| record(&trace, 1)).unwrap();
            match mode {
                DispatchMode::Emit => root.emit::<Number>(&0),
                DispatchMode::Bail => {
                    assert_eq!(root.bail::<Number>(&0), None);
                }
                _ => unreachable!(),
            }
        } else {
            let slot = target.clone();
            root.on_async::<AsyncNumber>(
                move |_| {
                    let effect = slot.lock().unwrap().take().unwrap();
                    drop(effect.dispose());
                    ready(None)
                },
                EventOptions::default(),
            )
            .unwrap();
            let trace = values.clone();
            *target.lock().unwrap() = Some(
                root.on_async::<AsyncNumber>(
                    move |_| ready(record(&trace, 0)),
                    EventOptions::default(),
                )
                .unwrap(),
            );
            let trace = values.clone();
            root.on_async::<AsyncNumber>(
                move |_| ready(record(&trace, 1)),
                EventOptions::default(),
            )
            .unwrap();
            match mode {
                DispatchMode::Parallel => host.finish(root.parallel::<AsyncNumber>(0)).unwrap(),
                DispatchMode::Serial => {
                    assert_eq!(host.finish(root.serial::<AsyncNumber>(0)).unwrap(), None);
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(*values.lock().unwrap(), [1], "{mode:?}");
    }
}

// EVT-12、S-EVT-2：派发期间的登记从后一次派发生效。
#[test]
fn all_modes_defer_added_listeners() {
    for mode in [
        DispatchMode::Emit,
        DispatchMode::Bail,
        DispatchMode::Parallel,
        DispatchMode::Serial,
    ] {
        let (root, host, values) = setup();
        let context = root.clone();
        let trace = values.clone();
        if matches!(mode, DispatchMode::Emit | DispatchMode::Bail) {
            root.on::<Number>(move |_| {
                let trace = trace.clone();
                context
                    .on::<Number>(move |value| record(&trace, *value))
                    .unwrap();
                None
            })
            .unwrap();
        } else {
            root.on_async::<AsyncNumber>(
                move |_| {
                    let trace = trace.clone();
                    context
                        .on_async::<AsyncNumber>(
                            move |value| ready(record(&trace, value)),
                            EventOptions::default(),
                        )
                        .unwrap();
                    ready(None)
                },
                EventOptions::default(),
            )
            .unwrap();
        }
        for value in [1, 2] {
            match mode {
                DispatchMode::Emit => root.emit::<Number>(&value),
                DispatchMode::Bail => {
                    root.bail::<Number>(&value);
                }
                DispatchMode::Parallel => host.finish(root.parallel::<AsyncNumber>(value)).unwrap(),
                DispatchMode::Serial => {
                    host.finish(root.serial::<AsyncNumber>(value)).unwrap();
                }
                _ => unreachable!(),
            }
            assert_eq!(
                *values.lock().unwrap(),
                if value == 1 { vec![] } else { vec![2] },
                "{mode:?}"
            );
        }
    }
}

// EVT-02、EVT-14：快照中已移除的中间件继续交给后继。
#[test]
fn waterfall_skips_removed_listener() {
    let (root, _, values) = setup();
    let target: Arc<Mutex<Option<Effect>>> = Arc::new(Mutex::new(None));
    let slot = target.clone();
    root.on_waterfall::<Water>(
        move |value, next| {
            let effect = slot.lock().unwrap().take().unwrap();
            drop(effect.dispose());
            Ok(value + next.run()?)
        },
        EventOptions::default(),
    )
    .unwrap();
    *target.lock().unwrap() = Some(
        root.on_waterfall::<Water>(
            |_, _| panic!("removed listener ran"),
            EventOptions::default(),
        )
        .unwrap(),
    );
    let trace = values.clone();
    root.on_waterfall::<Water>(
        move |value, next| {
            record(&trace, 1);
            Ok(value + next.run()?)
        },
        EventOptions::default(),
    )
    .unwrap();
    let trace = values.clone();
    assert_eq!(
        root.waterfall::<Water>(1, move || {
            record(&trace, 2);
            Ok(2)
        })
        .unwrap(),
        4
    );
    assert_eq!(*values.lock().unwrap(), [1, 2]);
}

// EVT-15、S-EVT-2。
#[test]
fn waterfall_defers_added_listener() {
    let (root, _, values) = setup();
    let context = root.clone();
    let trace = values.clone();
    root.on_waterfall::<Water>(
        move |_, next| {
            let trace = trace.clone();
            context
                .on_waterfall::<Water>(
                    move |value, next| {
                        record(&trace, value);
                        Ok(value + next.run()?)
                    },
                    EventOptions::default(),
                )
                .unwrap();
            next.run()
        },
        EventOptions::default(),
    )
    .unwrap();
    assert_eq!(root.waterfall::<Water>(1, || Ok(2)).unwrap(), 2);
    assert!(values.lock().unwrap().is_empty());
    assert_eq!(root.waterfall::<Water>(1, || Ok(2)).unwrap(), 3);
    assert_eq!(*values.lock().unwrap(), [1]);
}

// EVT-16、S-EVT-9：递归派发消费 once 后，外层继续运行后继。
#[test]
fn waterfall_recursion_consumes_once() {
    let (root, _, values) = setup();
    let context = root.clone();
    let nested = AtomicBool::new(false);
    root.on_waterfall::<Water>(
        move |value, next| {
            if !nested.swap(true, Ordering::SeqCst) {
                assert_eq!(context.waterfall::<Water>(value, || Ok(2))?, 4);
            }
            next.run()
        },
        EventOptions::default(),
    )
    .unwrap();
    let trace = values.clone();
    root.once_waterfall::<Water>(
        move |value, next| {
            record(&trace, value);
            Ok(value + next.run()?)
        },
        EventOptions::default(),
    )
    .unwrap();
    root.on_waterfall::<Water>(
        |value, next| Ok(value + next.run()?),
        EventOptions::default(),
    )
    .unwrap();
    assert_eq!(root.waterfall::<Water>(1, || Ok(2)).unwrap(), 3);
    assert_eq!(*values.lock().unwrap(), [1]);
}

// EVT-18、S-EVT-3、S-EVT-5。
#[test]
fn parallel_filters_and_aggregates_errors() {
    let (root, host, values) = setup();
    host.finish(root.parallel::<AsyncNumber>(0)).unwrap();
    let trace = values.clone();
    let listener = root
        .extend(true)
        .on_async::<AsyncNumber>(
            move |value| ready(record(&trace, value)),
            EventOptions::default(),
        )
        .unwrap();
    host.finish(root.parallel::<AsyncNumber>(1)).unwrap();
    host.finish(
        root.parallel_filtered::<AsyncNumber>(2, Some(&|ctx| !*ctx.extension::<bool>().unwrap())),
    )
    .unwrap();
    host.finish(
        root.parallel_filtered::<AsyncNumber>(3, Some(&|ctx| *ctx.extension::<bool>().unwrap())),
    )
    .unwrap();
    assert_eq!(*values.lock().unwrap(), [1, 3]);
    host.finish(listener.dispose()).unwrap();
    root.on_async::<AsyncNumber>(
        |_| Box::pin(async { Err(Error::new("test")) }),
        EventOptions::default(),
    )
    .unwrap();
    let clock = host.clone();
    let trace = values.clone();
    root.on_async::<AsyncNumber>(
        move |_| {
            let sleep = clock.sleep(Duration::from_millis(1));
            let trace = trace.clone();
            Box::pin(async move {
                sleep.await;
                record(&trace, 4);
                Err(Error::new("async"))
            })
        },
        EventOptions::default(),
    )
    .unwrap();
    let result = Arc::new(Mutex::new(None));
    let result_slot = result.clone();
    let task = root.parallel::<AsyncNumber>(4);
    host.spawn(Box::pin(async move {
        *result_slot.lock().unwrap() = Some(task.await);
    }));
    host.run_until_stalled();
    assert!(result.lock().unwrap().is_none());
    host.advance(Duration::from_millis(1));
    host.run_until_stalled();
    let error = result.lock().unwrap().take().unwrap().unwrap_err();
    assert_eq!(
        &*error.message,
        "one or more listeners of event \"custom-event\" failed"
    );
    assert_eq!(
        error
            .causes
            .iter()
            .map(|error| &*error.message)
            .collect::<Vec<_>>(),
        ["test", "async"]
    );
    assert_eq!(*values.lock().unwrap(), [1, 3, 4]);
}

// EVT-19：事件类型的名称进入聚合错误。
#[test]
fn parallel_labels_typed_event() {
    struct Named;
    impl Event for Named {
        const NAME: &'static str = "parallel";
        type Args = ();
        type Output = ();
        type Kind = AsyncKind;
    }
    let (root, host, _) = setup();
    root.on_async::<Named>(
        |_| Box::pin(async { Err(Error::new("test")) }),
        EventOptions::default(),
    )
    .unwrap();
    let error = host.finish(root.parallel::<Named>(())).unwrap_err();
    assert_eq!(
        &*error.message,
        "one or more listeners of event \"parallel\" failed"
    );
    assert_eq!(&*error.causes[0].message, "test");
}

// EVT-20、EVT-22、S-EVT-3、A-9：调用方接收同步 panic，后续内核操作继续生效。
#[test]
fn synchronous_filters_and_panic_propagation() {
    for bail in [false, true] {
        let (root, host, values) = setup();
        if bail {
            assert_eq!(root.bail::<Number>(&0), None);
        } else {
            root.emit::<Number>(&0);
        }
        let trace = values.clone();
        root.extend(true)
            .on::<Number>(move |value| record(&trace, *value))
            .unwrap();
        for value in [1, 2, 3] {
            let filter = |ctx: &Context| *ctx.extension::<bool>().unwrap() == (value != 2);
            let filter: Option<&dyn Fn(&Context) -> bool> = (value != 1).then_some(&filter);
            if bail {
                root.bail_filtered::<Number>(&value, filter);
            } else {
                root.emit_filtered::<Number>(&value, filter);
            }
        }
        assert_eq!(*values.lock().unwrap(), [1, 3]);
        let panic = root.on::<Number>(|_| panic!("message")).unwrap();
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                if bail {
                    root.bail::<Number>(&4);
                } else {
                    root.emit::<Number>(&4);
                }
            }))
            .is_err()
        );
        host.finish(panic.dispose()).unwrap();
        root.emit::<Number>(&5);
        assert_eq!(*values.lock().unwrap(), [1, 3, 4, 5]);
    }
}

// EVT-21、S-EVT-3、S-EVT-7。
#[test]
fn serial_filtering_errors_and_bail_result() {
    let (root, host, values) = setup();
    assert_eq!(host.finish(root.serial::<AsyncNumber>(0)).unwrap(), None);
    let trace = values.clone();
    root.extend(true)
        .on_async::<AsyncNumber>(
            move |value| ready(record(&trace, value)),
            EventOptions::default(),
        )
        .unwrap();
    host.finish(root.serial::<AsyncNumber>(1)).unwrap();
    host.finish(
        root.serial_filtered::<AsyncNumber>(2, Some(&|ctx| !*ctx.extension::<bool>().unwrap())),
    )
    .unwrap();
    host.finish(
        root.serial_filtered::<AsyncNumber>(3, Some(&|ctx| *ctx.extension::<bool>().unwrap())),
    )
    .unwrap();
    assert_eq!(*values.lock().unwrap(), [1, 3]);
    let failure = root
        .on_async::<AsyncNumber>(
            |_| Box::pin(async { Err(Error::new("message")) }),
            EventOptions::default(),
        )
        .unwrap();
    assert_eq!(
        &*host
            .finish(root.serial::<AsyncNumber>(4))
            .unwrap_err()
            .message,
        "message"
    );
    host.finish(failure.dispose()).unwrap();
    root.on_async::<AsyncNumber>(|value| ready(Some(value)), EventOptions::default())
        .unwrap();
    root.on_async::<AsyncNumber>(
        |_| panic!("serial continued after Some"),
        EventOptions::default(),
    )
    .unwrap();
    assert_eq!(host.finish(root.serial::<AsyncNumber>(5)).unwrap(), Some(5));
}

// S-EVT-1、S-EVT-3、S-EVT-6：头插顺序、global 过滤语义及 Some(0)。
#[test]
fn prepend_global_and_bail() {
    let (root, _, values) = setup();
    for (value, prepend) in [(1, false), (2, true), (3, false), (4, true)] {
        let trace = values.clone();
        root.on_with::<Number>(
            move |_| record(&trace, value),
            EventOptions {
                prepend,
                global: value == 3,
            },
        )
        .unwrap();
    }
    root.emit::<Number>(&0);
    assert_eq!(*values.lock().unwrap(), [4, 2, 1, 3]);
    values.lock().unwrap().clear();
    root.emit_filtered::<Number>(&0, Some(&|_| false));
    assert_eq!(*values.lock().unwrap(), [3]);
    root.on_with::<Number>(
        |_| Some(0),
        EventOptions {
            prepend: true,
            global: false,
        },
    )
    .unwrap();
    values.lock().unwrap().clear();
    assert_eq!(root.bail::<Number>(&0), Some(0));
    assert!(values.lock().unwrap().is_empty());
}

// EVT-23、S-EVT-8。
#[test]
fn waterfall_nesting_and_short_circuit() {
    let (root, _, values) = setup();
    for index in [1, 2] {
        let trace = values.clone();
        root.on_waterfall::<Water>(
            move |value, next| {
                record(&trace, index);
                Ok(value + next.run()?)
            },
            EventOptions::default(),
        )
        .unwrap();
    }
    assert_eq!(root.waterfall::<Water>(1, || Ok(2)).unwrap(), 4);
    assert_eq!(*values.lock().unwrap(), [1, 2]);
    values.lock().unwrap().clear();
    let trace = values.clone();
    root.on_waterfall::<Water>(
        move |value, _| {
            record(&trace, 3);
            Ok(value)
        },
        EventOptions::default(),
    )
    .unwrap();
    root.on_waterfall::<Water>(
        |_, _| panic!("waterfall continued after return"),
        EventOptions::default(),
    )
    .unwrap();
    assert_eq!(
        root.waterfall::<Water>(1, || panic!("terminal followed return"))
            .unwrap(),
        3
    );
    assert_eq!(*values.lock().unwrap(), [1, 2, 3]);
}

// EVT-24、S-EVT-8。
#[test]
fn waterfall_rejects_duplicate_next() {
    let (root, _, values) = setup();
    root.on_waterfall::<Water>(
        |_, next| {
            next.run()?;
            next.run()
        },
        EventOptions::default(),
    )
    .unwrap();
    let trace = values.clone();
    let error = root
        .waterfall::<Water>(1, move || {
            record(&trace, 2);
            Ok(2)
        })
        .unwrap_err();
    assert_eq!(&*error.message, "next() called multiple times");
    assert_eq!(*values.lock().unwrap(), [2]);
}

// EVT-25：外层延续及其克隆共享调用权。
#[test]
fn waterfall_rejects_outer_continuation() {
    let (root, _, values) = setup();
    let outer: Arc<Mutex<Option<Next<Water>>>> = Arc::new(Mutex::new(None));
    let slot = outer.clone();
    let trace = values.clone();
    root.on_waterfall::<Water>(
        move |_, next| {
            *slot.lock().unwrap() = Some(next.clone());
            record(&trace, 1);
            next.run()
        },
        EventOptions::default(),
    )
    .unwrap();
    let trace = values.clone();
    root.on_waterfall::<Water>(
        move |_, next| {
            record(&trace, 2);
            next.run()
        },
        EventOptions::default(),
    )
    .unwrap();
    let trace = values.clone();
    let error = root
        .waterfall::<Water>(1, move || {
            record(&trace, 3);
            let next = outer.lock().unwrap().as_ref().unwrap().clone();
            next.run()
        })
        .unwrap_err();
    assert_eq!(&*error.message, "next() called multiple times");
    assert_eq!(*values.lock().unwrap(), [1, 2, 3]);
}

// EVT-26：异步返回值完成后，延续仍保留已调用状态。
#[test]
fn waterfall_rejects_duplicate_after_await() {
    let (root, host, values) = setup();
    root.on_waterfall::<AsyncWater>(
        |value, next| {
            Ok(Box::pin(async move {
                let result = next.run()?.await?;
                let error = next.run().err().unwrap();
                assert_eq!(&*error.message, "next() called multiple times");
                Ok(value + result)
            }))
        },
        EventOptions::default(),
    )
    .unwrap();
    let trace = values.clone();
    let task = root
        .waterfall::<AsyncWater>(1, move || {
            record(&trace, 2);
            Ok(Box::pin(async { Ok(2) }))
        })
        .unwrap();
    assert_eq!(host.finish(task).unwrap(), 3);
    assert_eq!(*values.lock().unwrap(), [2]);
}

// EVT-27：每次嵌套派发独立保存监听器快照与延续。
#[test]
fn waterfall_supports_nested_async_calls() {
    let (root, host, values) = setup();
    let terminal_count = Arc::new(AtomicUsize::new(0));
    let context = root.clone();
    let trace = values.clone();
    let terminals = terminal_count.clone();
    root.on_waterfall::<AsyncWater>(
        move |value, next| {
            let context = context.clone();
            let trace = trace.clone();
            let terminals = terminals.clone();
            Ok(Box::pin(async move {
                record(&trace, value);
                let result = next.run()?.await?;
                if value == 1 {
                    let nested = context.waterfall::<AsyncWater>(2, move || {
                        terminals.fetch_add(1, Ordering::SeqCst);
                        Ok(Box::pin(async { Ok(2) }))
                    })?;
                    Ok(result + nested.await?)
                } else {
                    Ok(result)
                }
            }))
        },
        EventOptions::default(),
    )
    .unwrap();
    let terminals = terminal_count.clone();
    let task = root
        .waterfall::<AsyncWater>(1, move || {
            terminals.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(async { Ok(2) }))
        })
        .unwrap();
    assert_eq!(host.finish(task).unwrap(), 4);
    assert_eq!(*values.lock().unwrap(), [1, 2]);
    assert_eq!(terminal_count.load(Ordering::SeqCst), 2);
}

// S-EVT-10、T-EFF-6：接管返回的父效应包含实际监听器登记。
#[test]
fn internal_listener_takes_over_registration() {
    let (root, host, values) = setup();
    root.on::<InternalListener>(|request| {
        if !request.is::<Number>() {
            return None;
        }
        assert!(request.sync::<Number>().is_some());
        let request = request.clone();
        let context = request.context.clone();
        Some(
            context
                .effect("listener-owner", move |fx| {
                    fx.adopt(request.register(EventOptions {
                        prepend: true,
                        global: false,
                    })?)
                })
                .unwrap(),
        )
    })
    .unwrap();
    let trace = values.clone();
    let effect = root
        .on::<Number>(move |value| record(&trace, *value))
        .unwrap();
    root.emit::<Number>(&1);
    host.finish(effect.dispose()).unwrap();
    root.emit::<Number>(&2);
    assert_eq!(*values.lock().unwrap(), [1]);
    assert_eq!(root.listener_count::<Number>(), 0);
}

// S-EVT-12：观察记录携带模式、类型、参数、发起上下文和过滤状态。
#[test]
fn internal_dispatch_observes_public_events() {
    let (root, host, _) = setup();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let observed = observations.clone();
    let context = root.clone();
    root.on::<InternalDispatch>(move |dispatch| {
        observed
            .lock()
            .unwrap()
            .push((dispatch.mode, dispatch.name, dispatch.filtered));
        if dispatch.is::<Number>() {
            assert_eq!(dispatch.args::<Number>(), Some(&7));
            assert_eq!(dispatch.context.extension::<i32>().as_deref(), Some(&42));
            context.on::<Number>(|_| None).unwrap();
        }
        None
    })
    .unwrap();
    let source = root.extend(42_i32);
    source.emit_filtered::<Number>(&7, Some(&|_| true));
    source.bail::<Number>(&7);
    host.finish(source.parallel::<AsyncNumber>(8)).unwrap();
    host.finish(source.serial::<AsyncNumber>(9)).unwrap();
    source.waterfall::<Water>(1, || Ok(2)).unwrap();
    assert_eq!(
        observations.lock().unwrap().as_slice(),
        [
            (DispatchMode::Emit, "custom-event", true),
            (DispatchMode::Bail, "custom-event", false),
            (DispatchMode::Parallel, "custom-event", false),
            (DispatchMode::Serial, "custom-event", false),
            (DispatchMode::Waterfall, "test/waterfall", false),
        ]
    );
}
