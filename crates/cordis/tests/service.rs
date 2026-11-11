use cordis::{Clock, ComponentDef, Context, Error, FiberState, Inject, ServiceKey, Spawn};
use cordis_testkit::TestExecutor;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Foo;
impl ServiceKey for Foo {
    const NAME: &'static str = "foo";
    type Value = usize;
    type Meta = ();
}
struct Bar;
impl ServiceKey for Bar {
    const NAME: &'static str = "bar";
    type Value = usize;
    type Meta = ();
}
struct Qux;
impl ServiceKey for Qux {
    const NAME: &'static str = "qux";
    type Value = usize;
    type Meta = ();
}

/// T-COE-1、T-COE-3、T-COE-4、T-COE-5、T-CTX-3、T-CTX-4。
#[test]
fn svc01_provider_initialization_gates_consumers() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let loads = Arc::new(AtomicUsize::new(0));
    let count = loads.clone();
    let consumer = root
        .inject(Inject::new().with::<Foo>(), move |ctx| {
            let count = count.clone();
            async move {
                assert_eq!(*ctx.get::<Foo>()?, 7);
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .unwrap();
    let clock = host.clone();
    let provider = root
        .inject(Inject::new(), move |ctx| {
            let clock = clock.clone();
            async move {
                ctx.provide::<Foo>(Arc::new(7))?;
                clock.sleep(Duration::from_millis(10)).await;
                Ok(())
            }
        })
        .unwrap();
    host.run_until_stalled();
    assert_eq!(provider.state(), FiberState::Loading);
    assert_eq!(consumer.state(), FiberState::Pending);
    assert!(root.lookup::<Foo>().is_none());
    assert_eq!(loads.load(Ordering::SeqCst), 0);
    host.advance(Duration::from_millis(10));
    host.run_until_stalled();
    host.finish(consumer.settled()).unwrap();
    assert_eq!(consumer.state(), FiberState::Active);
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    host.finish(root.fiber().dispose()).unwrap();
}

struct Counter;
impl ServiceKey for Counter {
    const NAME: &'static str = "counter";
    type Value = AtomicUsize;
    type Meta = ();
}

/// T-CTX-1：调用方上下文决定服务方法建立的效应归属。
#[test]
fn svc02_scoped_counter_effect_belongs_to_declaring_consumer() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let counter = Arc::new(AtomicUsize::new(0));
    root.provide::<Counter>(counter.clone()).unwrap();
    let child = root
        .inject(Inject::new().with::<Counter>(), |ctx| async move {
            let service = ctx.get::<Counter>()?;
            let value = service.value();
            service.caller().effect("increment", move |fx| {
                value.fetch_add(1, Ordering::SeqCst);
                fx.defer(move || {
                    value.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
            })?;
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    host.finish(child.dispose()).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn svc03_root_access_retains_root_effect_ownership() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let counter = Arc::new(AtomicUsize::new(0));
    root.provide::<Counter>(counter.clone()).unwrap();
    let source = root.clone();
    let child = root
        .inject(Inject::new(), move |_| {
            let source = source.clone();
            async move {
                let service = source.get::<Counter>()?;
                let value = service.value();
                service.caller().effect("root increment", move |fx| {
                    value.fetch_add(1, Ordering::SeqCst);
                    fx.defer(move || {
                        value.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(child.dispose()).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    host.finish(root.fiber().dispose()).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[test]
fn svc04_service_lifecycle_restores_registration_snapshot() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let before = (root.service_count(), root.registry().len());
    let definition = ComponentDef::new("self", Inject::new(), |ctx, ()| async move {
        ctx.provide::<Foo>(Arc::new(1))?;
        ctx.inject(Inject::new().with::<Foo>(), |_| async { Ok(()) })?;
        Ok(())
    });
    let owner = root.mount_with(&definition, ()).unwrap();
    host.finish(owner.settled()).unwrap();
    host.run_until_stalled();
    assert_eq!((root.service_count(), root.registry().len()), (1, 2));
    host.finish(owner.dispose()).unwrap();
    assert_eq!((root.service_count(), root.registry().len()), before);
    host.finish(root.fiber().dispose()).unwrap();
}

/// T-LIF-10：依赖环停留在 Pending；满足链按提供顺序激活。
#[test]
fn svc05_multiple_dependencies_and_cycles() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let first = events.clone();
    let foo = root
        .inject(Inject::new().with::<Qux>(), move |ctx| {
            let first = first.clone();
            async move {
                first.lock().unwrap().push("foo");
                ctx.provide::<Foo>(Arc::new(1))?;
                Ok(())
            }
        })
        .unwrap();
    let next = events.clone();
    let bar = root
        .inject(Inject::new().with::<Foo>().with::<Qux>(), move |ctx| {
            let next = next.clone();
            async move {
                next.lock().unwrap().push("bar");
                ctx.provide::<Bar>(Arc::new(2))?;
                Ok(())
            }
        })
        .unwrap();
    host.run_until_stalled();
    assert_eq!(
        (foo.state(), bar.state()),
        (FiberState::Pending, FiberState::Pending)
    );
    let provided = events.clone();
    root.inject(Inject::new(), move |ctx| {
        let provided = provided.clone();
        async move {
            provided.lock().unwrap().push("qux");
            ctx.provide::<Qux>(Arc::new(3))?;
            Ok(())
        }
    })
    .unwrap();
    host.run_until_stalled();
    host.finish(bar.settled()).unwrap();
    assert_eq!(*events.lock().unwrap(), ["qux", "foo", "bar"]);
    host.finish(root.fiber().dispose()).unwrap();
    let foo = root
        .inject(Inject::new().with::<Bar>(), |ctx| async move {
            ctx.provide::<Foo>(Arc::new(1))?;
            Ok(())
        })
        .unwrap();
    let bar = root
        .inject(Inject::new().with::<Foo>(), |ctx| async move {
            ctx.provide::<Bar>(Arc::new(2))?;
            Ok(())
        })
        .unwrap();
    host.run_until_stalled();
    assert_eq!(
        (foo.state(), bar.state()),
        (FiberState::Pending, FiberState::Pending)
    );
    assert!(root.errors().is_empty());
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn svc06_svc07_svc08_dependency_cleanup_precedes_provider() {
    for action in ["dispose", "restart", "isolated"] {
        let host = TestExecutor::new();
        let root = Context::new(host.clone());
        let realm = cordis::Realm::new();
        let base = if action == "isolated" {
            root.isolate::<Foo>(realm)
        } else {
            root.clone()
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let stopped = events.clone();
        let provider = base
            .inject(Inject::new(), move |ctx| {
                let stopped = stopped.clone();
                async move {
                    ctx.provide::<Foo>(Arc::new(1))?;
                    ctx.effect("provider", move |fx| {
                        fx.defer(move || {
                            stopped.lock().unwrap().push("provider");
                            Ok(())
                        })
                    })?;
                    Ok(())
                }
            })
            .unwrap();
        let loads = Arc::new(AtomicUsize::new(0));
        let (clock, stopped, started) = (host.clone(), events.clone(), loads.clone());
        let consumer = base
            .inject(Inject::new().with::<Foo>(), move |ctx| {
                let (clock, stopped, started) = (clock.clone(), stopped.clone(), started.clone());
                async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    let read = ctx.clone();
                    ctx.effect("consumer", move |fx| {
                        fx.defer_async(move || async move {
                            clock.sleep(Duration::from_millis(10)).await;
                            assert_eq!(*read.get::<Foo>()?, 1);
                            stopped.lock().unwrap().push("consumer");
                            Ok(())
                        })
                    })?;
                    Ok(())
                }
            })
            .unwrap();
        host.run_until_stalled();
        host.finish(consumer.settled()).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        let task = if action == "restart" {
            provider.restart()
        } else {
            provider.dispose()
        };
        host.run_until_stalled();
        assert!(events.lock().unwrap().is_empty());
        host.advance(Duration::from_millis(10));
        host.finish(task).unwrap();
        assert_eq!(*events.lock().unwrap(), ["consumer", "provider"]);
        assert_eq!(
            loads.load(Ordering::SeqCst),
            if action == "restart" { 2 } else { 1 }
        );
        let task = root.fiber().dispose();
        host.run_until_stalled();
        host.advance(Duration::from_millis(10));
        host.finish(task).unwrap();
    }
}

/// T-LIF-2、T-LIF-6：新绑定身份改变纪元，在途加载完成后开始回退。
#[test]
fn fbr01_fbr02_inertial_loading_and_unloading() {
    for replace_during_load in [false, true] {
        let host = TestExecutor::new();
        let root = Context::new(host.clone());
        let old = root.provide::<Foo>(Arc::new(1)).unwrap();
        let clock = host.clone();
        let child = root
            .inject(Inject::new().with::<Foo>(), move |ctx| {
                let clock = clock.clone();
                async move {
                    clock.sleep(Duration::from_millis(1000)).await;
                    ctx.effect("slow cleanup", move |fx| {
                        fx.defer_async(move || async move {
                            clock.sleep(Duration::from_millis(1000)).await;
                            Ok(())
                        })
                    })?;
                    Ok(())
                }
            })
            .unwrap();
        host.run_until_stalled();
        host.advance(Duration::from_millis(400));
        host.run_until_stalled();
        assert_eq!(child.state(), FiberState::Loading);
        let withdraw = old.dispose();
        host.advance(Duration::from_millis(400));
        host.run_until_stalled();
        assert_eq!(child.state(), FiberState::Loading);
        if replace_during_load {
            root.provide::<Foo>(Arc::new(2)).unwrap();
        }
        host.advance(Duration::from_millis(400));
        host.run_until_stalled();
        assert_eq!(child.state(), FiberState::Unloading);
        if !replace_during_load {
            root.provide::<Foo>(Arc::new(2)).unwrap();
        }
        host.advance(Duration::from_millis(1000));
        host.run_until_stalled();
        assert_eq!(child.state(), FiberState::Loading);
        host.finish(withdraw).unwrap();
        host.advance(Duration::from_millis(1000));
        host.finish(child.settled()).unwrap();
        assert_eq!(*child.context().get::<Foo>().unwrap(), 2);
        let dispose = root.fiber().dispose();
        host.run_until_stalled();
        host.advance(Duration::from_millis(1000));
        host.finish(dispose).unwrap();
    }
}

#[test]
fn fbr03_active_consumer_returns_to_pending() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let provider = root
        .inject(Inject::new(), |ctx| async move {
            ctx.provide::<Foo>(Arc::new(1))?;
            Ok(())
        })
        .unwrap();
    let clock = host.clone();
    let child = root
        .inject(Inject::new().with::<Foo>(), move |ctx| {
            let clock = clock.clone();
            async move {
                clock.sleep(Duration::from_millis(10)).await;
                ctx.effect("delay", move |fx| {
                    fx.defer_async(move || async move {
                        clock.sleep(Duration::from_millis(10)).await;
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.run_until_stalled();
    assert_eq!(child.state(), FiberState::Loading);
    host.advance(Duration::from_millis(10));
    host.finish(child.settled()).unwrap();
    let task = provider.dispose();
    host.run_until_stalled();
    host.advance(Duration::from_millis(10));
    host.finish(task).unwrap();
    assert_eq!(child.state(), FiberState::Pending);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn fbr05_fbr06_failed_fiber_recovers_through_update() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = root.provide::<Foo>(Arc::new(1)).unwrap();
    let loads = Arc::new(AtomicUsize::new(0));
    let called = loads.clone();
    let component = ComponentDef::new(
        "recovery",
        Inject::new().with::<Foo>(),
        move |_, good: bool| {
            called.fetch_add(1, Ordering::SeqCst);
            async move {
                if good {
                    Ok(())
                } else {
                    Err(Error::new("boom"))
                }
            }
        },
    );
    let child = root.mount_with(&component, false).unwrap();
    assert!(host.finish(child.settled()).is_err());
    host.finish(old.dispose()).unwrap();
    root.provide::<Foo>(Arc::new(2)).unwrap();
    host.run_until_stalled();
    assert_eq!(child.state(), FiberState::Failed);
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    host.finish(child.update(true)).unwrap();
    assert_eq!(child.state(), FiberState::Active);
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn fbr12_pending_fiber_disposal_reaches_disposed() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let child = root
        .inject(Inject::new().with::<Foo>(), |_| async {
            Err(Error::new("unexpected load"))
        })
        .unwrap();
    assert_eq!(child.state(), FiberState::Pending);
    host.finish(child.dispose()).unwrap();
    assert_eq!(child.state(), FiberState::Disposed);
    root.provide::<Foo>(Arc::new(1)).unwrap();
    host.run_until_stalled();
    assert_eq!(child.state(), FiberState::Disposed);
    assert!(root.errors().is_empty());
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn fbr17_update_during_provider_reload_uses_latest_pair() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let provider_def =
        ComponentDef::new("provider", Inject::new(), |ctx, value: usize| async move {
            ctx.provide::<Foo>(Arc::new(value))?;
            Ok(())
        });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let events = seen.clone();
    let consumer_def = ComponentDef::new(
        "consumer",
        Inject::new().with::<Foo>(),
        move |ctx, mode: &'static str| {
            let events = events.clone();
            async move {
                events.lock().unwrap().push((*ctx.get::<Foo>()?, mode));
                Ok(())
            }
        },
    );
    let provider = root.mount_with(&provider_def, 1usize).unwrap();
    let child = root.mount_with(&consumer_def, "old").unwrap();
    host.run_until_stalled();
    host.finish(child.settled()).unwrap();
    let updating = provider.update(2usize);
    let updated = child.update("new");
    host.finish(updating).unwrap();
    host.finish(updated).unwrap();
    assert_eq!(*seen.lock().unwrap(), [(1, "old"), (2, "new")]);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn upd01_concurrent_updates_with_dependency_coalesce() {
    for dependent in [false, true] {
        let host = TestExecutor::new();
        let root = Context::new(host.clone());
        root.provide::<Foo>(Arc::new(1)).unwrap();
        let applied = Arc::new(Mutex::new(Vec::new()));
        let disposed = Arc::new(Mutex::new(Vec::new()));
        let (a, d, clock) = (applied.clone(), disposed.clone(), host.clone());
        let inject = if dependent {
            Inject::new().with::<Foo>()
        } else {
            Inject::new()
        };
        let component = ComponentDef::new("updates", inject, move |ctx, value: usize| {
            let (a, d, clock) = (a.clone(), d.clone(), clock.clone());
            async move {
                a.lock().unwrap().push(value);
                if dependent {
                    assert_eq!(*ctx.get::<Foo>()?, 1);
                }
                if value == 1 || value == 3 {
                    clock.sleep(Duration::from_millis(10)).await;
                }
                ctx.effect("config", move |fx| {
                    fx.defer(move || {
                        d.lock().unwrap().push(value);
                        Ok(())
                    })
                })?;
                Ok(())
            }
        });
        let child = root.mount_with(&component, 0usize).unwrap();
        host.finish(child.settled()).unwrap();
        let settled = Arc::new(AtomicUsize::new(0));
        let watch = |task: cordis::Task<cordis::Result>| {
            let settled = settled.clone();
            host.spawn(Box::pin(async move {
                task.await.unwrap();
                settled.fetch_add(1, Ordering::SeqCst);
            }));
        };
        watch(child.update(1usize));
        host.run_until_stalled();
        watch(child.update(2usize));
        watch(child.update(3usize));
        host.advance(Duration::from_millis(10));
        host.run_until_stalled();
        assert_eq!(*applied.lock().unwrap(), [0, 1, 3]);
        assert_eq!(*disposed.lock().unwrap(), [0, 1]);
        assert!(!child.is_settled());
        assert_eq!(settled.load(Ordering::SeqCst), 0);
        host.advance(Duration::from_millis(10));
        host.run_until_stalled();
        assert_eq!(settled.load(Ordering::SeqCst), 3);
        assert_eq!(*child.config::<usize>().unwrap(), 3);
        host.finish(root.fiber().dispose()).unwrap();
        assert_eq!(*disposed.lock().unwrap(), [0, 1, 3]);
    }
}

#[test]
fn upd08_pending_dependency_loads_latest_config() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let seen = values.clone();
    let definition = ComponentDef::new(
        "pending",
        Inject::new().with::<Foo>(),
        move |ctx, value: usize| {
            let seen = seen.clone();
            async move {
                assert_eq!(*ctx.get::<Foo>()?, 7);
                seen.lock().unwrap().push(value);
                Ok(())
            }
        },
    );
    let child = root.mount_with(&definition, 0usize).unwrap();
    host.finish(child.update(1usize)).unwrap();
    host.finish(child.update(2usize)).unwrap();
    assert!(values.lock().unwrap().is_empty());
    assert_eq!(child.state(), FiberState::Pending);
    root.provide::<Foo>(Arc::new(7)).unwrap();
    host.finish(child.settled()).unwrap();
    assert_eq!(*values.lock().unwrap(), [2]);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn cmp11_component_initialization_registers_release() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let lifecycle = Arc::new(Mutex::new(Vec::new()));
    let events = lifecycle.clone();
    let child = root
        .inject(Inject::new(), move |ctx| {
            let events = events.clone();
            async move {
                events.lock().unwrap().push("start");
                ctx.effect("stop", move |fx| {
                    fx.defer(move || {
                        events.lock().unwrap().push("stop");
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    assert_eq!(*lifecycle.lock().unwrap(), ["start"]);
    host.finish(child.dispose()).unwrap();
    assert_eq!(*lifecycle.lock().unwrap(), ["start", "stop"]);
}
