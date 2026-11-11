use cordis::{ComponentDef, Context, Error, Event, Fiber, Inject, ServiceKey, SyncKind, Task};
use cordis_testkit::TestExecutor;
use futures_util::{FutureExt, future::poll_fn};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Poll, Waker},
};

type Timeline = Arc<Mutex<Vec<String>>>;

#[derive(Clone, Default)]
struct Gate(Arc<Mutex<(bool, Vec<Waker>)>>);
impl Gate {
    fn wait(&self) -> Task {
        let gate = self.clone();
        Box::pin(poll_fn(move |cx| {
            let mut state = gate.0.lock().unwrap();
            if state.0 {
                Poll::Ready(())
            } else {
                if !state.1.iter().any(|w| w.will_wake(cx.waker())) {
                    state.1.push(cx.waker().clone());
                }
                Poll::Pending
            }
        }))
    }
    fn open(&self) {
        let waiters = {
            let mut state = self.0.lock().unwrap();
            state.0 = true;
            std::mem::take(&mut state.1)
        };
        for waiter in waiters {
            waiter.wake();
        }
    }
}

struct Foo;
impl ServiceKey for Foo {
    const NAME: &'static str = "foo";
    type Value = Resource;
    type Meta = ();
}
struct Bar;
impl ServiceKey for Bar {
    const NAME: &'static str = "bar";
    type Value = usize;
    type Meta = ();
}
struct Version;
impl ServiceKey for Version {
    const NAME: &'static str = "version";
    type Value = usize;
    type Meta = ();
}

struct Resource {
    closed: AtomicBool,
    timeline: Timeline,
}
impl Resource {
    fn read(&self) {
        let state = if self.closed.load(Ordering::SeqCst) {
            "closed"
        } else {
            "open"
        };
        self.timeline
            .lock()
            .unwrap()
            .push(format!("consumer read: {state}"));
    }
}

fn fixture() -> (Arc<TestExecutor>, Context, Fiber, Arc<Resource>, Timeline) {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let timeline = Arc::new(Mutex::new(Vec::new()));
    let resource = Arc::new(Resource {
        closed: AtomicBool::new(false),
        timeline: timeline.clone(),
    });
    let value = resource.clone();
    let definition = ComponentDef::new("provider", Inject::new(), move |ctx, ()| {
        let value = value.clone();
        async move {
            value.closed.store(false, Ordering::SeqCst);
            ctx.provide::<Foo>(value.clone())?;
            ctx.effect("close", move |fx| {
                fx.defer(move || {
                    value.closed.store(true, Ordering::SeqCst);
                    value
                        .timeline
                        .lock()
                        .unwrap()
                        .push("provider closed".into());
                    Ok(())
                })
            })?;
            Ok(())
        }
    });
    let provider = root.mount_with(&definition, ()).unwrap();
    host.finish(provider.settled()).unwrap();
    (host, root, provider, resource, timeline)
}

fn consumer(root: &Context, gate: Gate) -> Fiber {
    root.inject(Inject::new().with::<Foo>(), move |ctx| {
        let gate = gate.clone();
        async move {
            let cleanup = ctx.clone();
            ctx.effect("consumer", move |fx| {
                fx.defer_async(move || async move {
                    gate.wait().await;
                    cleanup.get::<Foo>()?.read();
                    Ok(())
                })
            })?;
            Ok(())
        }
    })
    .unwrap()
}

#[test]
fn svd01_releases_dependency_after_same_name_in_another_realm() {
    for action in ["consumer", "provider", "root"] {
        let (host, root, provider, value, _) = fixture();
        let consumer = root
            .inject(Inject::new().with::<Foo>(), move |ctx| {
                let value = value.clone();
                async move {
                    ctx.isolate::<Foo>(cordis::Realm::new())
                        .provide::<Foo>(value)?;
                    Ok(())
                }
            })
            .unwrap();
        host.finish(consumer.settled()).unwrap();
        match action {
            "consumer" => {
                host.finish(consumer.dispose()).unwrap();
                host.finish(provider.dispose()).unwrap();
            }
            "provider" => host.finish(provider.dispose()).unwrap(),
            _ => host.finish(root.fiber().dispose()).unwrap(),
        }
        assert_eq!(root.service_count(), 0);
        host.finish(root.fiber().dispose()).unwrap();
    }
}

#[test]
fn svd02_withdrawal_waits_for_consumer_after_registry_removal() {
    let (host, root, provider, _, timeline) = fixture();
    let provision = provider.context().provide::<Bar>(Arc::new(0)).unwrap();
    let gate = Gate::default();
    let waiting = gate.clone();
    let events = timeline.clone();
    let child = root
        .inject(Inject::new().with::<Bar>(), move |ctx| {
            let (waiting, events) = (waiting.clone(), events.clone());
            async move {
                ctx.effect("cleanup", move |fx| {
                    fx.defer_async(move || async move {
                        waiting.wait().await;
                        events.lock().unwrap().push("consumer finished".into());
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    let disposing = child.dispose();
    let withdrawing = provision.dispose();
    host.run_until_stalled();
    assert!(!child.is_settled());
    assert!(withdrawing.now_or_never().is_none());
    gate.open();
    host.finish(disposing).unwrap();
    host.finish(provision.dispose()).unwrap();
    assert_eq!(*timeline.lock().unwrap(), ["consumer finished"]);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn svd03_withdrawal_waits_for_its_own_binding() {
    let (host, root, provider, _, _) = fixture();
    let foo = provider.context().provide::<Bar>(Arc::new(1)).unwrap();
    provider.context().provide::<Version>(Arc::new(2)).unwrap();
    let gate = Gate::default();
    let waiting = gate.clone();
    let first = root
        .inject(Inject::new().with::<Bar>(), |_| async { Ok(()) })
        .unwrap();
    let second = root
        .inject(Inject::new().with::<Version>(), move |ctx| {
            let waiting = waiting.clone();
            async move {
                ctx.effect("hold", move |fx| {
                    fx.defer_async(move || async move {
                        waiting.wait().await;
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.run_until_stalled();
    let disposing = second.dispose();
    host.finish(foo.dispose()).unwrap();
    assert!(!second.is_settled());
    assert_eq!(first.state(), cordis::FiberState::Pending);
    gate.open();
    host.finish(disposing).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

/// T-COE-6：消费者清理期间，提供方资源持续可读。
#[test]
fn svd04_retains_resources_through_all_shutdown_orders() {
    for action in [
        "provider",
        "root",
        "provider first",
        "consumer first",
        "restart",
    ] {
        let (host, root, provider, resource, timeline) = fixture();
        let gate = Gate::default();
        let child = consumer(&root, gate.clone());
        host.run_until_stalled();
        host.finish(child.settled()).unwrap();
        let tasks = match action {
            "root" => vec![root.fiber().dispose()],
            "restart" => vec![provider.restart()],
            "consumer first" => vec![child.dispose(), provider.dispose()],
            "provider first" => vec![provider.dispose(), child.dispose()],
            _ => vec![provider.dispose()],
        };
        host.run_until_stalled();
        assert!(!resource.closed.load(Ordering::SeqCst), "{action}");
        gate.open();
        for task in tasks {
            host.finish(task).unwrap();
        }
        assert_eq!(
            *timeline.lock().unwrap(),
            ["consumer read: open", "provider closed"],
            "{action}"
        );
        if action == "restart" {
            host.finish(child.settled()).unwrap();
            assert!(child.context().get::<Foo>().is_ok());
        }
        host.finish(root.fiber().dispose()).unwrap();
    }
}

#[test]
fn svd05_retains_transitive_dependencies() {
    let (host, root, _, resource, timeline) = fixture();
    let middle = root
        .inject(Inject::new().with::<Foo>(), |ctx| async move {
            ctx.provide::<Bar>(Arc::new(0))?;
            let cleanup = ctx.clone();
            ctx.effect("read", move |fx| {
                fx.defer(move || {
                    cleanup.get::<Foo>()?.read();
                    Ok(())
                })
            })?;
            Ok(())
        })
        .unwrap();
    let gate = Gate::default();
    let (waiting, events) = (gate.clone(), timeline.clone());
    root.inject(Inject::new().with::<Bar>(), move |ctx| {
        let (waiting, events) = (waiting.clone(), events.clone());
        async move {
            ctx.effect("drain", move |fx| {
                fx.defer_async(move || {
                    events.lock().unwrap().push("consumer draining".into());
                    async move {
                        waiting.wait().await;
                        events.lock().unwrap().push("consumer finished".into());
                        Ok(())
                    }
                })
            })?;
            Ok(())
        }
    })
    .unwrap();
    host.finish(middle.settled()).unwrap();
    host.run_until_stalled();
    let task = root.fiber().dispose();
    host.run_until_stalled();
    assert!(!resource.closed.load(Ordering::SeqCst));
    gate.open();
    host.finish(task).unwrap();
    assert_eq!(
        *timeline.lock().unwrap(),
        [
            "consumer draining",
            "consumer finished",
            "consumer read: open",
            "provider closed"
        ]
    );
}

#[test]
fn svd06_retains_dependencies_during_setup_and_rollback() {
    for fail in [false, true] {
        let (host, root, provider, resource, timeline) = fixture();
        let gate = Gate::default();
        let (waiting, events) = (gate.clone(), timeline.clone());
        let child = root
            .inject(Inject::new().with::<Foo>(), move |ctx| {
                let (waiting, events) = (waiting.clone(), events.clone());
                async move {
                    events.lock().unwrap().push("setup started".into());
                    let (cleanup, disposed) = (ctx.clone(), events.clone());
                    ctx.effect("rollback", move |fx| {
                        fx.defer(move || {
                            cleanup.get::<Foo>()?.read();
                            disposed.lock().unwrap().push("setup rolled back".into());
                            Ok(())
                        })
                    })?;
                    waiting.wait().await;
                    ctx.get::<Foo>()?.read();
                    if fail {
                        Err(Error::new("setup failed"))
                    } else {
                        Ok(())
                    }
                }
            })
            .unwrap();
        host.run_until_stalled();
        let task = provider.dispose();
        host.run_until_stalled();
        assert!(!resource.closed.load(Ordering::SeqCst));
        gate.open();
        host.finish(task).unwrap();
        host.finish(child.dispose()).unwrap();
        assert_eq!(
            *timeline.lock().unwrap(),
            [
                "setup started",
                "consumer read: open",
                "consumer read: open",
                "setup rolled back",
                "provider closed"
            ]
        );
        assert_eq!(root.errors().len(), usize::from(fail));
        host.finish(root.fiber().dispose()).unwrap();
    }
}

#[test]
fn svd07_releases_dependencies_after_cleanup_error() {
    let (host, root, _, _, timeline) = fixture();
    let child = root
        .inject(Inject::new().with::<Foo>(), |ctx| async move {
            let cleanup = ctx.clone();
            ctx.effect("failure", move |fx| {
                fx.defer(move || {
                    cleanup.get::<Foo>()?.read();
                    Err(Error::new("cleanup failed"))
                })
            })?;
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
    assert_eq!(
        *timeline.lock().unwrap(),
        ["consumer read: open", "provider closed"]
    );
    assert!(
        root.errors()
            .iter()
            .any(|error| error.message.as_ref() == "cleanup failed")
    );
}

struct FinalEvent;
impl Event for FinalEvent {
    const NAME: &'static str = "final";
    type Args = ();
    type Output = ();
    type Kind = SyncKind;
}

#[test]
fn svd08_provider_listener_receives_final_consumer_event() {
    let (host, root, provider, _, _) = fixture();
    let received = Arc::new(AtomicBool::new(false));
    let flag = received.clone();
    provider
        .context()
        .on::<FinalEvent>(move |_| {
            flag.store(true, Ordering::SeqCst);
            None
        })
        .unwrap();
    let child = root
        .inject(Inject::new().with::<Foo>(), |ctx| async move {
            let cleanup = ctx.clone();
            ctx.effect("final event", move |fx| {
                fx.defer_async(move || async move {
                    cleanup.emit::<FinalEvent>(&());
                    Ok(())
                })
            })?;
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
    assert!(received.load(Ordering::SeqCst));
}

#[test]
fn svd09_independent_providers_and_sibling_effects_drain_concurrently() {
    let (host, root, _, resource, _) = fixture();
    let independent = Arc::new(AtomicBool::new(false));
    let flag = independent.clone();
    root.inject(Inject::new(), move |ctx| {
        let flag = flag.clone();
        async move {
            ctx.provide::<Bar>(Arc::new(0))?;
            ctx.effect("independent", move |fx| {
                fx.defer(move || {
                    flag.store(true, Ordering::SeqCst);
                    Ok(())
                })
            })?;
            Ok(())
        }
    })
    .unwrap();
    let gate = Gate::default();
    let child = consumer(&root, gate.clone());
    host.finish(child.settled()).unwrap();
    let sibling = Arc::new(AtomicBool::new(false));
    let flag = sibling.clone();
    child
        .context()
        .effect("sibling", move |fx| {
            fx.defer(move || {
                flag.store(true, Ordering::SeqCst);
                Ok(())
            })
        })
        .unwrap();
    let task = root.fiber().dispose();
    host.run_until_stalled();
    assert!(!resource.closed.load(Ordering::SeqCst));
    assert!(independent.load(Ordering::SeqCst));
    assert!(sibling.load(Ordering::SeqCst));
    gate.open();
    host.finish(task).unwrap();
}

#[test]
fn svd10_composed_provider_effect_retains_resource() {
    for owner_shutdown in [false, true] {
        let host = TestExecutor::new();
        let root = Context::new(host.clone());
        let timeline = Arc::new(Mutex::new(Vec::new()));
        let value = Arc::new(Resource {
            closed: AtomicBool::new(false),
            timeline: timeline.clone(),
        });
        let resource = value.clone();
        let provider = root
            .inject(Inject::new(), move |ctx| {
                let value = value.clone();
                async move {
                    let context = ctx.clone();
                    ctx.effect("composed provider", move |fx| {
                        let close = value.clone();
                        fx.adopt(context.effect("close", move |fx| {
                            fx.defer(move || {
                                close.closed.store(true, Ordering::SeqCst);
                                close
                                    .timeline
                                    .lock()
                                    .unwrap()
                                    .push("provider closed".into());
                                Ok(())
                            })
                        })?)?;
                        fx.adopt(context.provide::<Foo>(value)?.effect())
                    })?;
                    Ok(())
                }
            })
            .unwrap();
        let gate = Gate::default();
        let child = consumer(&root, gate.clone());
        host.finish(child.settled()).unwrap();
        host.run_until_stalled();
        let task = if owner_shutdown {
            root.fiber().dispose()
        } else {
            provider.dispose()
        };
        host.run_until_stalled();
        assert!(!resource.closed.load(Ordering::SeqCst));
        gate.open();
        host.finish(task).unwrap();
        assert_eq!(
            *timeline.lock().unwrap(),
            ["consumer read: open", "provider closed"]
        );
        host.finish(root.fiber().dispose()).unwrap();
    }
}

#[test]
fn svd11_child_in_composed_effect_reads_inherited_provision() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let timeline = Arc::new(Mutex::new(Vec::new()));
    let value = Arc::new(Resource {
        closed: AtomicBool::new(false),
        timeline: timeline.clone(),
    });
    let owner = root
        .inject(Inject::new(), move |ctx| {
            let value = value.clone();
            async move {
                ctx.provide::<Foo>(value)?;
                let context = ctx.clone();
                ctx.effect("children", move |fx| {
                    let child = context.inject(Inject::new(), |ctx| async move {
                        let cleanup = ctx.clone();
                        ctx.effect("read", move |fx| {
                            fx.defer(move || {
                                cleanup.get::<Foo>()?.read();
                                Ok(())
                            })
                        })?;
                        Ok(())
                    })?;
                    fx.defer_async(move || child.dispose())
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(owner.settled()).unwrap();
    host.run_until_stalled();
    host.finish(owner.dispose()).unwrap();
    assert_eq!(*timeline.lock().unwrap(), ["consumer read: open"]);
}

#[test]
fn svd12_ancestor_injection_lives_through_child_cleanup() {
    let (host, root, _, resource, timeline) = fixture();
    let gate = Gate::default();
    let waiting = gate.clone();
    let parent = root
        .inject(Inject::new().with::<Foo>(), move |ctx| {
            let waiting = waiting.clone();
            async move {
                ctx.inject(Inject::new(), move |ctx| {
                    let waiting = waiting.clone();
                    async move {
                        let cleanup = ctx.clone();
                        ctx.effect("child", move |fx| {
                            fx.defer_async(move || async move {
                                waiting.wait().await;
                                cleanup.get::<Foo>()?.read();
                                Ok(())
                            })
                        })?;
                        Ok(())
                    }
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(parent.settled()).unwrap();
    host.run_until_stalled();
    let task = root.fiber().dispose();
    host.run_until_stalled();
    assert!(!resource.closed.load(Ordering::SeqCst));
    gate.open();
    host.finish(task).unwrap();
    assert_eq!(
        *timeline.lock().unwrap(),
        ["consumer read: open", "provider closed"]
    );
}

#[test]
fn svd13_parent_provider_waits_for_explicit_child_dependency() {
    let (host, root, provider, resource, timeline) = fixture();
    let gate = Gate::default();
    let child = consumer(&provider.context(), gate.clone());
    host.finish(child.settled()).unwrap();
    let task = provider.dispose();
    host.run_until_stalled();
    assert!(!resource.closed.load(Ordering::SeqCst));
    gate.open();
    host.finish(task).unwrap();
    assert_eq!(
        *timeline.lock().unwrap(),
        ["consumer read: open", "provider closed"]
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn svd14_same_name_bindings_in_distinct_realms() {
    for root_shutdown in [false, true] {
        let (host, root, provider, resource, timeline) = fixture();
        let first = cordis::Realm::new();
        let second = cordis::Realm::new();
        provider
            .context()
            .isolate::<Foo>(first)
            .provide::<Foo>(resource.clone())
            .unwrap();
        provider
            .context()
            .isolate::<Foo>(second)
            .provide::<Foo>(resource.clone())
            .unwrap();
        let gate = Gate::default();
        let child = consumer(&root.isolate::<Foo>(first), gate.clone());
        host.finish(child.settled()).unwrap();
        let task = if root_shutdown {
            root.fiber().dispose()
        } else {
            provider.dispose()
        };
        host.run_until_stalled();
        assert_eq!(child.state(), cordis::FiberState::Unloading);
        assert!(!resource.closed.load(Ordering::SeqCst));
        gate.open();
        host.finish(task).unwrap();
        assert_eq!(
            *timeline.lock().unwrap(),
            ["consumer read: open", "provider closed"]
        );
        host.finish(root.fiber().dispose()).unwrap();
    }
}

#[test]
fn svd15_old_withdrawal_finishes_while_new_load_waits() {
    let (host, root, provider, _, _) = fixture();
    let old = provider.context().provide::<Version>(Arc::new(1)).unwrap();
    let cleanup = Gate::default();
    let setup = Gate::default();
    let versions = Arc::new(Mutex::new(Vec::new()));
    let (clean, ready, seen) = (cleanup.clone(), setup.clone(), versions.clone());
    let child = root
        .inject(Inject::new().with::<Version>(), move |ctx| {
            let (clean, ready, seen) = (clean.clone(), ready.clone(), seen.clone());
            async move {
                let version = *ctx.get::<Version>()?;
                seen.lock().unwrap().push(version);
                if version == 2 {
                    ready.wait().await;
                }
                let context = ctx.clone();
                ctx.effect("old cleanup", move |fx| {
                    fx.defer_async(move || async move {
                        clean.wait().await;
                        seen.lock().unwrap().push(*context.get::<Version>()?);
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    let withdrawing = old.dispose();
    provider.context().provide::<Version>(Arc::new(2)).unwrap();
    cleanup.open();
    host.finish(withdrawing).unwrap();
    assert_eq!(child.state(), cordis::FiberState::Loading);
    assert_eq!(*provider.context().get::<Version>().unwrap(), 2);
    assert_eq!(*versions.lock().unwrap(), [1, 1, 2]);
    setup.open();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn svd16_provider_waits_for_current_and_retired_bindings() {
    for retired_first in [false, true] {
        let (host, root, provider, resource, timeline) = fixture();
        let old = provider.context().provide::<Bar>(Arc::new(0)).unwrap();
        let current_gate = Gate::default();
        let old_gate = Gate::default();
        let first = consumer(&root, current_gate.clone());
        let (waiting, events) = (old_gate.clone(), timeline.clone());
        let second = root
            .inject(Inject::new().with::<Bar>(), move |ctx| {
                let (waiting, events) = (waiting.clone(), events.clone());
                async move {
                    ctx.effect("retired", move |fx| {
                        fx.defer_async(move || async move {
                            waiting.wait().await;
                            events.lock().unwrap().push("consumer finished".into());
                            Ok(())
                        })
                    })?;
                    Ok(())
                }
            })
            .unwrap();
        host.finish(first.settled()).unwrap();
        host.finish(second.settled()).unwrap();
        let withdrawing = old.dispose();
        provider.context().provide::<Bar>(Arc::new(1)).unwrap();
        let task = provider.dispose();
        host.run_until_stalled();
        assert!(!resource.closed.load(Ordering::SeqCst));
        let (one, two) = if retired_first {
            (&old_gate, &current_gate)
        } else {
            (&current_gate, &old_gate)
        };
        one.open();
        host.run_until_stalled();
        assert!(!resource.closed.load(Ordering::SeqCst));
        two.open();
        host.finish(withdrawing).unwrap();
        host.finish(task).unwrap();
        let expected = if retired_first {
            vec![
                "consumer finished",
                "consumer read: open",
                "provider closed",
            ]
        } else {
            vec![
                "consumer read: open",
                "consumer finished",
                "provider closed",
            ]
        };
        assert_eq!(*timeline.lock().unwrap(), expected);
        host.finish(root.fiber().dispose()).unwrap();
    }
}

#[test]
fn svd17_binding_replacement_during_consumer_setup() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = root.provide::<Version>(Arc::new(1)).unwrap();
    let gate = Gate::default();
    let versions = Arc::new(Mutex::new(Vec::new()));
    let (waiting, seen) = (gate.clone(), versions.clone());
    let child = root
        .inject(Inject::new().with::<Version>(), move |ctx| {
            let (waiting, seen) = (waiting.clone(), seen.clone());
            async move {
                seen.lock().unwrap().push(*ctx.get::<Version>()? as isize);
                waiting.wait().await;
                let context = ctx.clone();
                ctx.effect("version", move |fx| {
                    fx.defer(move || {
                        seen.lock()
                            .unwrap()
                            .push(-(*context.get::<Version>()? as isize));
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.run_until_stalled();
    let task = old.dispose();
    root.provide::<Version>(Arc::new(2)).unwrap();
    gate.open();
    host.finish(child.settled()).unwrap();
    host.finish(task).unwrap();
    assert_eq!(*child.context().get::<Version>().unwrap(), 2);
    host.finish(child.dispose()).unwrap();
    assert_eq!(*versions.lock().unwrap(), [1, -1, 2, -2]);
    host.finish(root.fiber().dispose()).unwrap();
}
