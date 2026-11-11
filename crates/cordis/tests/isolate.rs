use cordis::{Context, Event, FiberState, Inject, Realm, ServiceKey, SyncKind};
use cordis_testkit::TestExecutor;
use std::sync::{Arc, Mutex};

struct Foo;
impl ServiceKey for Foo {
    const NAME: &'static str = "foo";
    type Value = &'static str;
    type Meta = ();
}

/// T-COE-7、T-COE-13：派生隔离上下文保留父视图。
#[test]
fn iso01_own_provisions_follow_their_realm() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let owner = root
        .inject(Inject::new(), |ctx| async move {
            let isolated = ctx.isolate::<Foo>(Realm::new());
            isolated.provide::<Foo>(Arc::new("inner"))?;
            assert_eq!(*isolated.get::<Foo>()?, "inner");
            assert!(ctx.get::<Foo>().is_err());
            isolated.inject(Inject::new(), |child| async move {
                assert_eq!(*child.get::<Foo>()?, "inner");
                Ok(())
            })?;
            ctx.inject(Inject::new(), |child| async move {
                assert!(child.get::<Foo>().is_err());
                Ok(())
            })?;
            Ok(())
        })
        .unwrap();
    host.finish(owner.settled()).unwrap();
    host.run_until_stalled();
    assert!(root.errors().is_empty());
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn iso02_explicit_isolation_stops_ancestor_injection() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new("outer")).unwrap();
    let results = Arc::new(Mutex::new(Vec::new()));
    let seen = results.clone();
    let owner = root
        .inject(Inject::new().with::<Foo>(), move |ctx| {
            let seen = seen.clone();
            async move {
                let contexts = [
                    ctx.clone(),
                    ctx.isolate::<Foo>(Realm::new()),
                    ctx.isolate::<Foo>(ctx.realm::<Foo>()),
                ];
                for context in contexts {
                    let seen = seen.clone();
                    context.inject(Inject::new(), move |child| {
                        let seen = seen.clone();
                        async move {
                            seen.lock().unwrap().push(
                                child
                                    .get::<Foo>()
                                    .map(|value| (*value).to_string())
                                    .unwrap_or_else(|error| error.message.to_string()),
                            );
                            Ok(())
                        }
                    })?;
                }
                Ok(())
            }
        })
        .unwrap();
    host.finish(owner.settled()).unwrap();
    host.run_until_stalled();
    assert_eq!(
        *results.lock().unwrap(),
        [
            "outer",
            "cannot get property \"foo\" without inject",
            "cannot get property \"foo\" without inject"
        ]
    );
    host.finish(root.fiber().dispose()).unwrap();
}

/// T-COE-8：独立域各自加载，共享域同步响应。
#[test]
fn iso03_iso04_independent_and_shared_realms() {
    for shared in [false, true] {
        let host = TestExecutor::new();
        let root = Context::new(host.clone());
        let realm = Realm::new();
        let first = root.isolate::<Foo>(realm);
        let second = root.isolate::<Foo>(if shared { realm } else { Realm::new() });
        let consumers = [&root, &first, &second].map(|ctx| {
            ctx.inject(Inject::new().with::<Foo>(), |_| async { Ok(()) })
                .unwrap()
        });
        let outer = root.provide::<Foo>(Arc::new("outer")).unwrap();
        host.run_until_stalled();
        assert_eq!(
            consumers
                .iter()
                .map(|fiber| fiber.state())
                .collect::<Vec<_>>(),
            [FiberState::Active, FiberState::Pending, FiberState::Pending]
        );
        let inner = first.provide::<Foo>(Arc::new("inner")).unwrap();
        host.run_until_stalled();
        assert_eq!(*root.get::<Foo>().unwrap(), "outer");
        assert_eq!(*first.get::<Foo>().unwrap(), "inner");
        assert_eq!(
            consumers[2].state(),
            if shared {
                FiberState::Active
            } else {
                FiberState::Pending
            }
        );
        host.finish(inner.dispose()).unwrap();
        assert_eq!(consumers[1].state(), FiberState::Pending);
        assert_eq!(consumers[2].state(), FiberState::Pending);
        assert_eq!(consumers[0].state(), FiberState::Active);
        host.finish(outer.dispose()).unwrap();
        assert_eq!(consumers[0].state(), FiberState::Pending);
        host.finish(root.fiber().dispose()).unwrap();
    }
}

struct Message;
impl Event for Message {
    const NAME: &'static str = "isolated message";
    type Args = ();
    type Output = ();
    type Kind = SyncKind;
}

#[test]
fn iso05_service_event_filter_uses_isolation_realm() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let isolated = root.isolate::<Foo>(Realm::new());
    let received = Arc::new(Mutex::new(Vec::new()));
    for (context, name) in [(&root, "outer"), (&isolated, "inner")] {
        let received = received.clone();
        context
            .on::<Message>(move |_| {
                received.lock().unwrap().push(name);
                None
            })
            .unwrap();
    }
    let realm = isolated.realm::<Foo>();
    isolated.emit_filtered::<Message>(&(), Some(&|listener| listener.realm::<Foo>() == realm));
    assert_eq!(*received.lock().unwrap(), ["inner"]);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn iso06_own_provisions_and_committed_dependency_remain_distinct() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new("outer")).unwrap();
    let child = root
        .inject(Inject::new().with::<Foo>(), |ctx| async move {
            let first = ctx.isolate::<Foo>(Realm::new());
            let second = ctx.isolate::<Foo>(Realm::new());
            let old = first.provide::<Foo>(Arc::new("first"))?;
            second.provide::<Foo>(Arc::new("second"))?;
            assert_eq!(*ctx.get::<Foo>()?, "outer");
            assert_eq!(*first.get::<Foo>()?, "first");
            assert_eq!(*second.get::<Foo>()?, "second");
            old.dispose().await?;
            assert_eq!(*ctx.get::<Foo>()?, "outer");
            assert!(first.get::<Foo>().is_err());
            assert_eq!(*second.get::<Foo>()?, "second");
            let read = ctx.clone();
            ctx.effect("dependency cleanup", move |fx| {
                fx.defer(move || {
                    assert_eq!(*read.get::<Foo>()?, "outer");
                    Ok(())
                })
            })?;
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(child.dispose()).unwrap();
    assert!(root.errors().is_empty());
    host.finish(root.fiber().dispose()).unwrap();
}
