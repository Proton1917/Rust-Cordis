use cordis::{
    AnyArc, ComponentDef, Context, DispatchMode, Effect, Error, Event, EventOptions, FiberState,
    Inject, InternalDispatch, InternalGet, InternalListener, InternalSet, ServiceKey, SyncKind,
};
use cordis_testkit::TestExecutor;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Typed;
impl ServiceKey for Typed {
    const NAME: &'static str = "typed@1";
    type Value = usize;
    type Meta = ();
}

struct Dynamic;
impl ServiceKey for Dynamic {
    const NAME: &'static str = "dynamic@1";
    type Value = usize;
    type Meta = ();
}

// K-1、K-2：运行时名称由宿主持有，命名依赖读取类型化提供的实际擦除表示。
#[test]
fn owned_names_and_mixed_dependency_registration() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let typed = root.provide::<Typed>(Arc::new(7)).unwrap();
    let name = format!("{}@{}", "dynamic", 1);
    let dynamic = root.provide_named(name.clone(), Arc::new(9usize)).unwrap();
    drop(name);
    let component_name = String::from("dynamic-consumer");
    let definition = ComponentDef::new(
        component_name.clone(),
        Inject::new()
            .with_named(Typed::NAME)
            .with_named(Dynamic::NAME),
        |ctx, ()| async move {
            assert_eq!(
                **ctx
                    .get_named(Typed::NAME)?
                    .downcast_ref::<Arc<usize>>()
                    .unwrap(),
                7
            );
            assert_eq!(
                *ctx.get_named(Dynamic::NAME)?
                    .downcast_ref::<usize>()
                    .unwrap(),
                9
            );
            assert!(ctx.get::<Dynamic>().is_err());
            Ok(())
        },
    );
    drop(component_name);
    let child = root.mount_with(&definition, ()).unwrap();
    host.finish(child.settled()).unwrap();
    assert_eq!(definition.name(), "dynamic-consumer");
    assert_eq!(child.name(), "dynamic-consumer");
    assert!(root.provide_named(Typed::NAME, Arc::new(1usize)).is_err());
    assert!(root.provide::<Dynamic>(Arc::new(1)).is_err());
    assert!(root.lookup::<Dynamic>().is_none());
    dynamic.set(Arc::new(Arc::new(11usize))).unwrap();
    assert_eq!(*root.get::<Dynamic>().unwrap(), 11);
    host.finish(typed.dispose()).unwrap();
    assert_eq!(child.state(), FiberState::Pending);
    assert!(root.provide_named(Typed::NAME, Arc::new(1usize)).is_err());
    host.finish(root.fiber().dispose()).unwrap();
}

// K-2、T-COE-6、T-COE-8：命名替换沿依赖顺序清理，同名隔离域保持原纪元。
#[test]
fn named_replacement_reloads_only_its_realm() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Typed>(Arc::new(5)).unwrap();
    let trace = Arc::new(Mutex::new(Vec::new()));
    let make = |value| {
        let trace = trace.clone();
        ComponentDef::new(
            format!("provider-{value}"),
            Inject::new(),
            move |ctx, ()| {
                let trace = trace.clone();
                async move {
                    ctx.effect("provider-resource", move |fx| {
                        fx.defer(move || {
                            trace.lock().unwrap().push(("provider", value));
                            Ok(())
                        })
                    })?;
                    ctx.provide_named(Dynamic::NAME, Arc::new(value))?;
                    Ok(())
                }
            },
        )
    };
    let old = make(1usize);
    let next = make(2usize);
    let provider = root.mount_with(&old, ()).unwrap();
    host.finish(provider.settled()).unwrap();
    let seen = trace.clone();
    let consumer = root
        .inject(
            Inject::new().with_named(Dynamic::NAME).with::<Typed>(),
            move |ctx| {
                let seen = seen.clone();
                async move {
                    assert_eq!(*ctx.get::<Typed>()?, 5);
                    let value = *ctx
                        .get_named(Dynamic::NAME)?
                        .downcast_ref::<usize>()
                        .unwrap();
                    seen.lock().unwrap().push(("loaded", value));
                    ctx.effect("consumer-resource", move |fx| {
                        fx.defer(move || {
                            seen.lock().unwrap().push(("consumer", value));
                            Ok(())
                        })
                    })?;
                    Ok(())
                }
            },
        )
        .unwrap();
    host.finish(consumer.settled()).unwrap();
    let isolated = root.isolate_named(Dynamic::NAME, cordis::Realm::new());
    isolated
        .provide_named(Dynamic::NAME, Arc::new(20usize))
        .unwrap();
    let unaffected = isolated
        .inject(Inject::new().with_named(Dynamic::NAME), |ctx| async move {
            assert_eq!(
                *ctx.get_named(Dynamic::NAME)?
                    .downcast_ref::<usize>()
                    .unwrap(),
                20
            );
            Ok(())
        })
        .unwrap();
    host.finish(unaffected.settled()).unwrap();
    let epoch = unaffected.epoch();
    root.registry().replace(&old, &next);
    host.finish(provider.settled()).unwrap();
    host.finish(consumer.settled()).unwrap();
    assert_eq!(
        *trace.lock().unwrap(),
        [
            ("loaded", 1),
            ("consumer", 1),
            ("provider", 1),
            ("loaded", 2)
        ]
    );
    assert_eq!(unaffected.epoch(), epoch);
    assert_eq!(
        *root
            .isolate::<Dynamic>(isolated.realm_named(Dynamic::NAME))
            .get_named(Dynamic::NAME)
            .unwrap()
            .downcast_ref::<usize>()
            .unwrap(),
        20
    );
    host.finish(root.fiber().dispose()).unwrap();
}

// K-2、T-COE-10–T-COE-13：声明元数据被外层、内层依次覆盖，视图变化保留纪元。
#[test]
fn named_metadata_preserves_declaration_and_view_rules() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide_named(Dynamic::NAME, Arc::new(1usize)).unwrap();
    let child = root
        .inject(
            Inject::new().with_named_meta(Dynamic::NAME, Arc::new(String::from("declared"))),
            |_| async { Ok(()) },
        )
        .unwrap();
    host.finish(child.settled()).unwrap();
    let context = child.context();
    let epoch = child.epoch();
    assert_eq!(
        context
            .named_meta(Dynamic::NAME)
            .unwrap()
            .downcast_ref::<String>()
            .unwrap(),
        "declared"
    );
    let outer = context.intercept_named(Dynamic::NAME, Arc::new(String::from("outer")));
    let inner = outer.intercept_named(Dynamic::NAME, Arc::new(String::from("inner")));
    assert_eq!(
        outer
            .named_meta(Dynamic::NAME)
            .unwrap()
            .downcast_ref::<String>()
            .unwrap(),
        "outer"
    );
    assert_eq!(
        inner
            .named_meta(Dynamic::NAME)
            .unwrap()
            .downcast_ref::<String>()
            .unwrap(),
        "inner"
    );
    assert_eq!(child.epoch(), epoch);
    let boundary = context.isolate_named(Dynamic::NAME, root.realm_named(Dynamic::NAME));
    assert!(boundary.named_meta(Dynamic::NAME).is_none());
    assert!(boundary.get_named(Dynamic::NAME).is_err());
    let inherited = context
        .inject(Inject::new(), |ctx| async move {
            assert_eq!(
                ctx.named_meta(Dynamic::NAME)
                    .unwrap()
                    .downcast_ref::<String>()
                    .unwrap(),
                "declared"
            );
            Ok(())
        })
        .unwrap();
    host.finish(inherited.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

// K-2、S-SVC-10：命名读写进入既有瀑布，原始载荷及重入上下文保持可用。
#[test]
fn named_access_uses_service_interceptors() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let provision = root.provide_named(Dynamic::NAME, Arc::new(2usize)).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = reads.clone();
    root.on_waterfall::<InternalGet>(
        move |access, next| {
            let mut result = next.run()?;
            if access.name.as_ref() == Dynamic::NAME {
                assert_eq!(access.context.service_count(), 1);
                observed.fetch_add(1, Ordering::SeqCst);
                result.value = Arc::new(result.value.downcast_ref::<usize>().unwrap() + 10);
            }
            Ok(result)
        },
        EventOptions::default(),
    )
    .unwrap();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let observed = writes.clone();
    root.on_waterfall::<InternalSet>(
        move |(access, value), next| {
            assert_eq!(access.name.as_ref(), Dynamic::NAME);
            observed
                .lock()
                .unwrap()
                .push(*value.downcast_ref::<usize>().unwrap());
            next.run()
        },
        EventOptions::default(),
    )
    .unwrap();
    assert_eq!(
        *root
            .get_named(Dynamic::NAME)
            .unwrap()
            .downcast_ref::<usize>()
            .unwrap(),
        12
    );
    provision.set(Arc::new(4usize)).unwrap();
    assert_eq!(
        *root
            .get_named(Dynamic::NAME)
            .unwrap()
            .downcast_ref::<usize>()
            .unwrap(),
        14
    );
    assert_eq!(*writes.lock().unwrap(), [4]);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    host.finish(root.fiber().dispose()).unwrap();
}

struct TypedEvent;
impl Event for TypedEvent {
    const NAME: &'static str = "typed-event@1";
    type Args = ();
    type Output = ();
    type Kind = SyncKind;
}

// K-1、K-3、S-EVT-9：一次性命名监听器在递归前撤销，观察载荷保持原始 AnyArc。
#[test]
fn named_events_reenter_and_release_listeners() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let name = format!("{}@{}", "changed", 1);
    let observations = Arc::new(Mutex::new(Vec::new()));
    let seen = observations.clone();
    root.on::<InternalDispatch>(move |dispatch| {
        if dispatch.is_named() {
            seen.lock().unwrap().push((
                dispatch.mode,
                dispatch.name.to_string(),
                *dispatch.args.downcast_ref::<usize>().unwrap(),
            ));
        }
        None
    })
    .unwrap();
    let taken = Arc::new(AtomicUsize::new(0));
    let observed = taken.clone();
    root.on::<InternalListener>(move |request| {
        if request.is_named() {
            observed.fetch_add(1, Ordering::SeqCst);
            Some(request.register(EventOptions::default()).unwrap())
        } else {
            None
        }
    })
    .unwrap();
    let values = Arc::new(Mutex::new(Vec::new()));
    let seen = values.clone();
    let nested = root.clone();
    let event = name.clone();
    root.once_named(name.clone(), move |value| {
        seen.lock()
            .unwrap()
            .push(*value.downcast_ref::<usize>().unwrap());
        let payload: AnyArc = Arc::new(2usize);
        nested.emit_named(event.clone(), &payload)?;
        Ok(None)
    })
    .unwrap();
    let seen = values.clone();
    let listener = root
        .on_named(name.clone(), move |value| {
            seen.lock()
                .unwrap()
                .push(10 + value.downcast_ref::<usize>().unwrap());
            Ok(None)
        })
        .unwrap();
    drop(name);
    let payload: AnyArc = Arc::new(1usize);
    root.emit_named("changed@1", &payload).unwrap();
    assert_eq!(*values.lock().unwrap(), [1, 12, 11]);
    assert_eq!(taken.load(Ordering::SeqCst), 2);
    host.finish(listener.dispose()).unwrap();
    root.emit_named("changed@1", &payload).unwrap();
    assert_eq!(*values.lock().unwrap(), [1, 12, 11]);
    assert_eq!(
        *observations.lock().unwrap(),
        [
            (DispatchMode::Emit, "changed@1".to_string(), 1),
            (DispatchMode::Emit, "changed@1".to_string(), 2),
            (DispatchMode::Emit, "changed@1".to_string(), 1),
        ]
    );
    host.finish(root.fiber().dispose()).unwrap();
}

// K-3、S-EVT-2、S-EVT-5：快照跳过已撤回回调，同名同步与异步族各自派发。
#[test]
fn named_async_dispatch_uses_snapshot_and_aggregates_failures() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let sync_calls = Arc::new(AtomicUsize::new(0));
    let calls = sync_calls.clone();
    root.on_named("event@1", move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    })
    .unwrap();
    let target: Arc<Mutex<Option<Effect>>> = Arc::new(Mutex::new(None));
    let removed = target.clone();
    root.on_named_async(
        "event@1",
        move |_| {
            drop(removed.lock().unwrap().take().unwrap().dispose());
            Box::pin(async { Err(Error::new("first")) })
        },
        EventOptions::default(),
    )
    .unwrap();
    *target.lock().unwrap() = Some(
        root.on_named_async(
            "event@1",
            |_| panic!("removed callback ran"),
            EventOptions::default(),
        )
        .unwrap(),
    );
    let observed = Arc::new(AtomicUsize::new(0));
    let calls = observed.clone();
    root.on_named_async(
        "event@1",
        move |value| {
            let calls = calls.clone();
            Box::pin(async move {
                calls.store(*value.downcast_ref::<usize>().unwrap(), Ordering::SeqCst);
                Err(Error::new("last"))
            })
        },
        EventOptions::default(),
    )
    .unwrap();
    let error = host
        .finish(root.parallel_named("event@1", Arc::new(7usize)))
        .unwrap_err();
    assert_eq!(
        error.message.as_ref(),
        "one or more listeners of event \"event@1\" failed"
    );
    assert_eq!(
        error
            .causes
            .iter()
            .map(|error| error.message.as_ref())
            .collect::<Vec<_>>(),
        ["first", "last"]
    );
    assert_eq!(observed.load(Ordering::SeqCst), 7);
    assert_eq!(sync_calls.load(Ordering::SeqCst), 0);
    let payload: AnyArc = Arc::new(9usize);
    root.emit_named("event@1", &payload).unwrap();
    assert_eq!(sync_calls.load(Ordering::SeqCst), 1);
    host.finish(root.fiber().dispose()).unwrap();
}

// ADR-0011：事件名称的首次登记方式约束后续提供，同步和异步命名族共享方式。
#[test]
fn typed_and_named_event_registration_conflict_both_ways() {
    let host = TestExecutor::new();
    let typed = Context::new(host.clone());
    typed.on::<TypedEvent>(|_| None).unwrap();
    assert!(typed.on_named(TypedEvent::NAME, |_| Ok(None)).is_err());
    assert!(
        typed
            .on_named_async(
                TypedEvent::NAME,
                |_| Box::pin(async { Ok(None) }),
                EventOptions::default()
            )
            .is_err()
    );
    let named = Context::new(host.clone());
    named.on_named(TypedEvent::NAME, |_| Ok(None)).unwrap();
    assert!(named.on::<TypedEvent>(|_| None).is_err());
    host.finish(typed.fiber().dispose()).unwrap();
    host.finish(named.fiber().dispose()).unwrap();
}

struct ReenterOnDrop {
    context: Context,
    dropped: Arc<AtomicUsize>,
}

// K-3：命名同步监听器的业务错误经 Result 返回，撤回后派发恢复。
#[test]
fn named_sync_dispatch_returns_listener_error() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let failed = root
        .on_named("failure@1", |_| Err(Error::new("plugin failure")))
        .unwrap();
    let payload: AnyArc = Arc::new(());
    let error = root.emit_named("failure@1", &payload).unwrap_err();
    assert_eq!(error.message.as_ref(), "plugin failure");
    host.finish(failed.dispose()).unwrap();
    root.emit_named("failure@1", &payload).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

impl Drop for ReenterOnDrop {
    fn drop(&mut self) {
        self.context.service_count();
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

// K-4、A-3：连续三次替换释放旧闭包，析构重入发生在内核锁外。
#[test]
fn replacement_releases_each_retired_definition() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let dropped = Arc::new(AtomicUsize::new(0));
    let make = |name: &str| {
        let capture = Arc::new(ReenterOnDrop {
            context: root.clone(),
            dropped: dropped.clone(),
        });
        let weak = Arc::downgrade(&capture);
        let definition = ComponentDef::new(name.to_string(), Inject::new(), move |_, ()| {
            let capture = capture.clone();
            async move {
                drop(capture);
                Ok(())
            }
        });
        (definition, weak)
    };
    let (a, old_a) = make("a");
    let (b, old_b) = make("b");
    let (c, old_c) = make("c");
    let d = ComponentDef::new("d", Inject::new(), |_, ()| async { Ok(()) });
    let fiber = root.mount_with(&a, ()).unwrap();
    host.finish(fiber.settled()).unwrap();
    let mut current = a;
    for (next, capture) in [(b, old_a), (c, old_b), (d, old_c)] {
        root.registry().replace(&current, &next);
        drop(current);
        host.finish(fiber.settled()).unwrap();
        assert!(capture.upgrade().is_none());
        current = next;
    }
    assert_eq!(dropped.load(Ordering::SeqCst), 3);
    assert_eq!(fiber.name(), "d");
    host.finish(root.fiber().dispose()).unwrap();
}

// K-4、S-REG-5：当前转发终点在无实例时继续支持旧别名挂载。
#[test]
fn canonical_definition_survives_last_instance_for_alias_mount() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = ComponentDef::new("old", Inject::new(), |_, ()| async { Ok(()) });
    let applied = Arc::new(AtomicUsize::new(0));
    let calls = applied.clone();
    let next = ComponentDef::new("next", Inject::new(), move |_, ()| {
        calls.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    root.registry().replace(&old, &next);
    drop(next);
    let first = root.mount_with(&old, ()).unwrap();
    host.finish(first.settled()).unwrap();
    host.finish(first.dispose()).unwrap();
    assert!(root.registry().is_empty());
    let second = root.mount_with(&old, ()).unwrap();
    host.finish(second.settled()).unwrap();
    assert_eq!(second.name(), "next");
    assert_eq!(applied.load(Ordering::SeqCst), 2);
    host.finish(root.fiber().dispose()).unwrap();
}
