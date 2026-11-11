use cordis::{
    AsyncKind, ComponentDef, Context, Effect, Event, EventOptions, Fiber, Inject, InternalUpdate,
};
use cordis_testkit::TestExecutor;
use std::sync::{Arc, Mutex};

struct Signal;
impl Event for Signal {
    const NAME: &'static str = "component-event";
    type Args = ();
    type Output = ();
    type Kind = AsyncKind;
}

// EVT-13、S-EVT-2：前置监听器释放组件后，当前快照跳过该组件的监听器。
#[test]
fn serial_skips_listener_disposed_with_component() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let child: Arc<Mutex<Option<Fiber>>> = Arc::new(Mutex::new(None));
    let target = child.clone();
    root.on_async::<Signal>(
        move |_| {
            let task = target.lock().unwrap().as_ref().unwrap().dispose();
            Box::pin(async move {
                task.await?;
                Ok(None)
            })
        },
        EventOptions::default(),
    )
    .unwrap();
    let component = ComponentDef::new("listener", Inject::new(), |ctx, ()| async move {
        ctx.on_async::<Signal>(
            |_| panic!("disposed component listener ran"),
            EventOptions::default(),
        )?;
        Ok(())
    });
    let fiber = root.mount_with(&component, ()).unwrap();
    host.finish(fiber.settled()).unwrap();
    *child.lock().unwrap() = Some(fiber);
    assert_eq!(host.finish(root.serial::<Signal>(())).unwrap(), None);
}

// EVT-17、S-EVT-11：纤程更新快照跳过同次更新中释放的监听器。
#[test]
fn update_skips_removed_fiber_listener() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let loaded = Arc::new(Mutex::new(Vec::new()));
    let updated = Arc::new(Mutex::new(Vec::new()));
    let loads = loaded.clone();
    let updates = updated.clone();
    let component = ComponentDef::new("updater", Inject::new(), move |ctx, config: i32| {
        let loads = loads.clone();
        let updates = updates.clone();
        async move {
            loads.lock().unwrap().push(config);
            let target: Arc<Mutex<Option<Effect>>> = Arc::new(Mutex::new(None));
            let slot = target.clone();
            ctx.on_waterfall::<InternalUpdate>(
                move |_, next| {
                    let effect = slot.lock().unwrap().take().unwrap();
                    drop(effect.dispose());
                    next.run()
                },
                EventOptions::default(),
            )?;
            *target.lock().unwrap() = Some(ctx.on_waterfall::<InternalUpdate>(
                |_, _| panic!("removed update listener ran"),
                EventOptions::default(),
            )?);
            ctx.on_waterfall::<InternalUpdate>(
                move |config, next| {
                    updates
                        .lock()
                        .unwrap()
                        .push(*config.downcast_ref::<i32>().unwrap());
                    next.run()
                },
                EventOptions::default(),
            )?;
            Ok(())
        }
    });
    let fiber = root.mount_with(&component, 0_i32).unwrap();
    host.finish(fiber.settled()).unwrap();
    host.finish(fiber.update(1_i32)).unwrap();
    assert_eq!(*loaded.lock().unwrap(), [0, 1]);
    assert_eq!(*updated.lock().unwrap(), [1]);
}

// EVT-28、S-EVT-11：每次重载释放旧更新监听器。
#[test]
fn update_listeners_are_replaced_on_reload() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let updated = Arc::new(Mutex::new(Vec::new()));
    let updates = updated.clone();
    let component = ComponentDef::new("updater", Inject::new(), move |ctx, _: i32| {
        let updates = updates.clone();
        async move {
            ctx.on_waterfall::<InternalUpdate>(
                move |config, next| {
                    updates
                        .lock()
                        .unwrap()
                        .push(*config.downcast_ref::<i32>().unwrap());
                    next.run()
                },
                EventOptions::default(),
            )?;
            Ok(())
        }
    });
    let fiber = root.mount_with(&component, 0_i32).unwrap();
    host.finish(fiber.settled()).unwrap();
    for value in [1_i32, 2, 3] {
        host.finish(fiber.update(value)).unwrap();
    }
    assert_eq!(*updated.lock().unwrap(), [1, 2, 3]);
}

// EVT-29、S-EVT-11：释放句柄撤销所属纤程的更新监听器。
#[test]
fn update_listener_can_be_disposed() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let updated = Arc::new(Mutex::new(Vec::new()));
    let updates = updated.clone();
    let listener = Arc::new(Mutex::new(None));
    let slot = listener.clone();
    let component = ComponentDef::new("updater", Inject::new(), move |ctx, _: i32| {
        let updates = updates.clone();
        let slot = slot.clone();
        async move {
            *slot.lock().unwrap() = Some(ctx.on_waterfall::<InternalUpdate>(
                move |config, next| {
                    updates
                        .lock()
                        .unwrap()
                        .push(*config.downcast_ref::<i32>().unwrap());
                    next.run()
                },
                EventOptions::default(),
            )?);
            Ok(())
        }
    });
    let fiber = root.mount_with(&component, 0_i32).unwrap();
    host.finish(fiber.settled()).unwrap();
    let effect = listener.lock().unwrap().take().unwrap();
    host.finish(effect.dispose()).unwrap();
    host.finish(fiber.update(1_i32)).unwrap();
    assert!(updated.lock().unwrap().is_empty());
}

// EVT-30、S-EVT-1、S-EVT-11：头插保持局部监听器顺序，释放后继续下一次更新。
#[test]
fn update_prepended_listeners_run_first() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let order = Arc::new(Mutex::new(Vec::new()));
    let calls = order.clone();
    let listener = Arc::new(Mutex::new(None));
    let slot = listener.clone();
    let component = ComponentDef::new("updater", Inject::new(), move |ctx, _: i32| {
        let calls = calls.clone();
        let slot = slot.clone();
        async move {
            for (name, prepend) in [("a", false), ("b", true), ("c", false), ("d", true)] {
                let calls = calls.clone();
                let effect = ctx.on_waterfall::<InternalUpdate>(
                    move |_, next| {
                        calls.lock().unwrap().push(name);
                        next.run()
                    },
                    EventOptions {
                        prepend,
                        global: false,
                    },
                )?;
                if name == "b" {
                    *slot.lock().unwrap() = Some(effect);
                }
            }
            Ok(())
        }
    });
    let fiber = root.mount_with(&component, 0_i32).unwrap();
    host.finish(fiber.settled()).unwrap();
    host.finish(fiber.update(1_i32)).unwrap();
    assert_eq!(*order.lock().unwrap(), ["d", "b", "a", "c"]);
    order.lock().unwrap().clear();
    let effect = listener.lock().unwrap().take().unwrap();
    host.finish(effect.dispose()).unwrap();
    host.finish(fiber.update(2_i32)).unwrap();
    assert_eq!(*order.lock().unwrap(), ["d", "a", "c"]);
}

// EVT-31、S-EVT-3、S-EVT-11：global 与纤程局部登记按各自归属恢复。
#[test]
fn global_update_listener_survives_child_reload() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let global = Arc::new(Mutex::new(Vec::new()));
    let local = Arc::new(Mutex::new(Vec::new()));
    let calls = global.clone();
    root.on_waterfall::<InternalUpdate>(
        move |config, next| {
            calls
                .lock()
                .unwrap()
                .push(*config.downcast_ref::<i32>().unwrap());
            next.run()
        },
        EventOptions {
            global: true,
            prepend: false,
        },
    )
    .unwrap();
    let calls = local.clone();
    let component = ComponentDef::new("updater", Inject::new(), move |ctx, _: i32| {
        let calls = calls.clone();
        async move {
            ctx.on_waterfall::<InternalUpdate>(
                move |config, next| {
                    calls
                        .lock()
                        .unwrap()
                        .push(*config.downcast_ref::<i32>().unwrap());
                    next.run()
                },
                EventOptions::default(),
            )?;
            Ok(())
        }
    });
    let fiber = root.mount_with(&component, 0_i32).unwrap();
    host.finish(fiber.settled()).unwrap();
    host.finish(fiber.update(1_i32)).unwrap();
    host.finish(fiber.update(2_i32)).unwrap();
    assert_eq!(*global.lock().unwrap(), [1, 2]);
    assert_eq!(*local.lock().unwrap(), [1, 2]);
}
