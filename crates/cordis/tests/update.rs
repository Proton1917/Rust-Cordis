use cordis::{ComponentDef, Context, Error, FiberState, Inject, InternalUpdate, RawConfig};
use cordis_testkit::TestExecutor;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

// UPD-02、T-LIF-3：执行前的更新合并到最新配置。
#[test]
fn superseded_loads_are_skipped_before_apply() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(Mutex::new(Vec::new()));
    let values = applied.clone();
    let def = ComponentDef::new("queued", Inject::new(), move |_, value: usize| {
        values.lock().unwrap().push(value);
        async { Ok(()) }
    });
    let fiber = root.mount_with(&def, 1usize).unwrap();
    let first = fiber.update(2usize);
    let second = fiber.update(3usize);
    host.finish(first).unwrap();
    host.finish(second).unwrap();
    assert_eq!(*applied.lock().unwrap(), [3]);
    assert_eq!(fiber.state(), FiberState::Active);
    host.finish(fiber.dispose()).unwrap();
}

// UPD-03、T-LIF-5、T-LIF-6：卸载持续至清理完成，再加载最新配置。
#[test]
fn updates_coalesce_during_unloading() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(Mutex::new(Vec::new()));
    let values = applied.clone();
    let def = ComponentDef::new("unloading", Inject::new(), move |ctx, value: usize| {
        values.lock().unwrap().push(value);
        async move {
            let cleanup_ctx = ctx.clone();
            ctx.effect("cleanup", move |fx| {
                fx.defer_async(move || async move {
                    if value == 0 {
                        cleanup_ctx.sleep(Duration::from_millis(10)).await;
                    }
                    Ok(())
                })
            })?;
            Ok(())
        }
    });
    let fiber = root.mount_with(&def, 0usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    let first = fiber.update(1usize);
    host.run_until_stalled();
    let second = fiber.update(2usize);
    assert_eq!(*applied.lock().unwrap(), [0]);
    assert_eq!(fiber.state(), FiberState::Unloading);
    host.advance(Duration::from_millis(10));
    host.finish(first).unwrap();
    host.finish(second).unwrap();
    assert_eq!(*applied.lock().unwrap(), [0, 2]);
    assert_eq!(fiber.state(), FiberState::Active);
    host.finish(fiber.dispose()).unwrap();
}

// UPD-04、T-LIF-2、T-LIF-6：相同配置的重启及 ABA 更新仍推进纪元。
#[test]
fn restart_and_configuration_aba_supersede_loading() {
    for restart in [false, true] {
        let host = TestExecutor::new();
        let root = Context::new(host.clone());
        let applied = Arc::new(Mutex::new(Vec::new()));
        let values = applied.clone();
        let def = ComponentDef::new("aba", Inject::new(), move |ctx, value: usize| {
            values.lock().unwrap().push(value);
            async move {
                ctx.sleep(Duration::from_millis(10)).await;
                Ok(())
            }
        });
        let fiber = root.mount_with(&def, 1usize).unwrap();
        host.run_until_stalled();
        let waiting = if restart {
            vec![fiber.restart()]
        } else {
            vec![fiber.update(2usize), fiber.update(1usize)]
        };
        host.advance(Duration::from_millis(10));
        host.run_until_stalled();
        assert_eq!(*applied.lock().unwrap(), [1, 1]);
        host.advance(Duration::from_millis(10));
        for wait in waiting {
            host.finish(wait).unwrap();
        }
        assert_eq!(*fiber.config::<usize>().unwrap(), 1);
        assert_eq!(fiber.state(), FiberState::Active);
        host.finish(fiber.dispose()).unwrap();
    }
}

// UPD-05、T-LIF-4：过时加载的失败执行清理，随后加载当前配置。
#[test]
fn superseded_failure_preserves_latest_target() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(Mutex::new(Vec::new()));
    let disposed = Arc::new(Mutex::new(Vec::new()));
    let started = applied.clone();
    let ended = disposed.clone();
    let def = ComponentDef::new("superseded", Inject::new(), move |ctx, value: usize| {
        started.lock().unwrap().push(value);
        let ended = ended.clone();
        async move {
            ctx.effect("resource", move |fx| {
                fx.defer(move || {
                    ended.lock().unwrap().push(value);
                    Ok(())
                })
            })?;
            if value == 1 {
                ctx.sleep(Duration::from_millis(10)).await;
                return Err(Error::new("superseded load"));
            }
            Ok(())
        }
    });
    let fiber = root.mount_with(&def, 1usize).unwrap();
    host.run_until_stalled();
    let update = fiber.update(2usize);
    host.advance(Duration::from_millis(10));
    host.finish(update).unwrap();
    assert_eq!(*applied.lock().unwrap(), [1, 2]);
    assert_eq!(*disposed.lock().unwrap(), [1]);
    let errors = root.errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message.as_ref(), "superseded load");
    assert_eq!(fiber.state(), FiberState::Active);
    host.finish(fiber.dispose()).unwrap();
    assert_eq!(*disposed.lock().unwrap(), [1, 2]);
}

// UPD-06、S-FBR-21：并发更新的全部等待者收到最终加载错误。
#[test]
fn concurrent_waiters_receive_latest_failure() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(Mutex::new(Vec::new()));
    let disposed = Arc::new(Mutex::new(Vec::new()));
    let started = applied.clone();
    let ended = disposed.clone();
    let def = ComponentDef::new("latest", Inject::new(), move |ctx, value: usize| {
        started.lock().unwrap().push(value);
        let ended = ended.clone();
        async move {
            ctx.effect("resource", move |fx| {
                fx.defer(move || {
                    ended.lock().unwrap().push(value);
                    Ok(())
                })
            })?;
            if value == 1 {
                ctx.sleep(Duration::from_millis(10)).await;
            }
            if value == 2 {
                return Err(Error::new("latest load failed"));
            }
            Ok(())
        }
    });
    let fiber = root.mount_with(&def, 0usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    let first = fiber.update(1usize);
    host.run_until_stalled();
    let second = fiber.update(2usize);
    host.advance(Duration::from_millis(10));
    assert_eq!(
        host.finish(first).unwrap_err().message.as_ref(),
        "latest load failed"
    );
    assert_eq!(
        host.finish(second).unwrap_err().message.as_ref(),
        "latest load failed"
    );
    assert_eq!(*applied.lock().unwrap(), [0, 1, 2]);
    assert_eq!(*disposed.lock().unwrap(), [0, 1, 2]);
    assert_eq!(fiber.state(), FiberState::Failed);
    host.finish(fiber.dispose()).unwrap();
}

// UPD-07、T-LIF-7、T-EFF-4：步骤完成后保留其逆变换，并在守卫失效时结束推进。
#[test]
fn cooperative_step_keeps_final_cleanup_and_stops_continuation() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let trace = events.clone();
    let def = ComponentDef::new("steps", Inject::new(), move |ctx, value: usize| {
        let trace = trace.clone();
        async move {
            trace.lock().unwrap().push(format!("load {value}"));
            if value != 1 {
                return Ok(());
            }
            ctx.sleep(Duration::from_millis(10)).await;
            let cleanup = trace.clone();
            ctx.effect("final yield", |fx| {
                fx.defer(move || {
                    cleanup.lock().unwrap().push("dispose 1".into());
                    Ok(())
                })
            })?;
            if !ctx.is_current() {
                return Ok(());
            }
            trace.lock().unwrap().push("outdated continuation".into());
            Ok(())
        }
    });
    let fiber = root.mount_with(&def, 1usize).unwrap();
    host.run_until_stalled();
    let update = fiber.update(2usize);
    host.advance(Duration::from_millis(10));
    host.finish(update).unwrap();
    assert_eq!(*events.lock().unwrap(), ["load 1", "dispose 1", "load 2"]);
    assert_eq!(fiber.state(), FiberState::Active);
    host.finish(fiber.dispose()).unwrap();
}

// UPD-09：加载期间监听器接受更新，保留当前加载与最新原始配置。
#[test]
fn accepted_update_during_loading_preserves_current_load() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::new(Mutex::new(Vec::new()));
    let started = applied.clone();
    let updates = received.clone();
    let def = ComponentDef::new("accepted", Inject::new(), move |ctx, value: usize| {
        started.lock().unwrap().push(value);
        let updates = updates.clone();
        async move {
            ctx.on_waterfall::<InternalUpdate>(
                move |raw: RawConfig, _| {
                    updates
                        .lock()
                        .unwrap()
                        .push(*raw.downcast_ref::<usize>().unwrap());
                    Ok(Box::pin(async { Ok(()) }))
                },
                Default::default(),
            )?;
            ctx.sleep(Duration::from_millis(10)).await;
            Ok(())
        }
    });
    let fiber = root.mount_with(&def, 1usize).unwrap();
    host.run_until_stalled();
    host.finish(fiber.update(2usize)).unwrap();
    host.advance(Duration::from_millis(10));
    host.finish(fiber.settled()).unwrap();
    assert_eq!(*applied.lock().unwrap(), [1]);
    assert_eq!(*received.lock().unwrap(), [2]);
    assert_eq!(*fiber.config::<usize>().unwrap(), 2);
    assert_eq!(fiber.state(), FiberState::Active);
    host.finish(fiber.dispose()).unwrap();
}

// UPD-10、T-LIF-1：释放优先于排队配置，执行中的加载完成后进入清理。
#[test]
fn disposal_prevents_queued_configuration_load() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(Mutex::new(Vec::new()));
    let disposed = Arc::new(Mutex::new(Vec::new()));
    let started = applied.clone();
    let ended = disposed.clone();
    let def = ComponentDef::new("disposed", Inject::new(), move |ctx, value: usize| {
        started.lock().unwrap().push(value);
        let ended = ended.clone();
        async move {
            ctx.effect("resource", move |fx| {
                fx.defer(move || {
                    ended.lock().unwrap().push(value);
                    Ok(())
                })
            })?;
            ctx.sleep(Duration::from_millis(10)).await;
            Ok(())
        }
    });
    let fiber = root.mount_with(&def, 1usize).unwrap();
    host.run_until_stalled();
    let updating = fiber.update(2usize);
    let disposing = fiber.dispose();
    host.advance(Duration::from_millis(10));
    host.finish(updating).unwrap();
    host.finish(disposing).unwrap();
    assert_eq!(*applied.lock().unwrap(), [1]);
    assert_eq!(*disposed.lock().unwrap(), [1]);
    assert_eq!(fiber.state(), FiberState::Disposed);
}
