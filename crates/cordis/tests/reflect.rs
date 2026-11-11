use cordis::{ComponentDef, Context, FiberState, Inject, InternalStatus, ServiceKey};
use cordis_testkit::TestExecutor;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
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
struct Collision;
impl ServiceKey for Collision {
    const NAME: &'static str = "foo";
    type Value = bool;
    type Meta = ();
}

#[test]
fn rfl01_context_extension_preserves_context_operations() {
    let host = TestExecutor::new();
    let context = Context::new(host.clone()).extend(String::from("derived"));
    context.provide::<Foo>(Arc::new(1)).unwrap();
    assert_eq!(*context.get::<Foo>().unwrap(), 1);
    assert_eq!(context.extension::<String>().unwrap().as_str(), "derived");
    host.finish(context.fiber().dispose()).unwrap();
}

/// T-COE-2、T-CTX-2：登记冲突与声明访问在运行边界报告。
#[test]
fn rfl02_access_and_registration_contracts() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let child = root
        .inject(Inject::new(), |ctx| async move {
            assert_eq!(
                ctx.get::<Bar>().unwrap_err().message.as_ref(),
                "cannot get property \"bar\" without inject"
            );
            let provision = ctx.provide::<Foo>(Arc::new(1))?;
            let error = ctx.provide::<Foo>(Arc::new(2)).err().unwrap();
            assert!(
                error
                    .message
                    .contains("service \"foo\" has been registered")
            );
            provision.set(Arc::new(3))?;
            assert_eq!(*ctx.get::<Foo>()?, 3);
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    assert!(
        root.provide::<Collision>(Arc::new(true))
            .err()
            .unwrap()
            .message
            .contains("another key")
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn rfl03_service_injection_and_inherited_extension() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(1)).unwrap();
    let parent = root
        .inject(Inject::new().with::<Foo>(), |ctx| async move {
            ctx.extend(2usize).inject(Inject::new(), |ctx| async move {
                assert_eq!(*ctx.extension::<usize>().unwrap(), 2);
                assert_eq!(*ctx.get::<Foo>()?, 1);
                Ok(())
            })?;
            Ok(())
        })
        .unwrap();
    host.finish(parent.settled()).unwrap();
    host.run_until_stalled();
    assert!(root.errors().is_empty());
    assert!(root.lookup::<Bar>().is_none());
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn rfl04_access_error_retains_call_location() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let child = root
        .inject(Inject::new(), |ctx| async move {
            let expected = line!() + 1;
            let error = ctx.get::<Bar>().unwrap_err();
            assert!(
                error
                    .locations
                    .iter()
                    .any(|site| site.file().ends_with("reflect.rs") && site.line() == expected)
            );
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn rfl05_disposed_injection_reports_inactive_access() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(1)).unwrap();
    let child = root
        .inject(Inject::new().with::<Foo>(), |_| async { Ok(()) })
        .unwrap();
    host.finish(child.settled()).unwrap();
    let context = child.context();
    assert_eq!(*context.get::<Foo>().unwrap(), 1);
    host.finish(child.dispose()).unwrap();
    assert_eq!(
        context.get::<Foo>().unwrap_err().message.as_ref(),
        "cannot get required service \"foo\" in inactive context"
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn rfl06_disposed_during_notification_keeps_component_unexecuted() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(Mutex::new(Vec::new()));
    let seen = applied.clone();
    let definition = ComponentDef::new(
        "consumer",
        Inject::new().with::<Foo>(),
        move |_, id: usize| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(id);
                Ok(())
            }
        },
    );
    let first = root.mount_with(&definition, 1usize).unwrap();
    let second = root.mount_with(&definition, 2usize).unwrap();
    let (watched, disposed) = (first.clone(), second.clone());
    root.on::<InternalStatus>(move |(fiber, _)| {
        if *fiber == watched && fiber.state() == FiberState::Loading {
            drop(disposed.dispose());
        }
        None
    })
    .unwrap();
    root.provide::<Foo>(Arc::new(1)).unwrap();
    host.finish(first.settled()).unwrap();
    assert_eq!(*applied.lock().unwrap(), [1]);
    assert_eq!(second.state(), FiberState::Disposed);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn availability_check_reenters_kernel_and_updates_dependents() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let enabled = Arc::new(AtomicBool::new(false));
    let ready = enabled.clone();
    let context = root.clone();
    let provision = root
        .provide_with_check::<Foo>(Arc::new(7), move || {
            assert!(context.service_count() > 0);
            ready.load(Ordering::SeqCst)
        })
        .unwrap();
    let loads = Arc::new(AtomicUsize::new(0));
    let count = loads.clone();
    let child = root
        .inject(Inject::new().with::<Foo>(), move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        })
        .unwrap();
    host.run_until_stalled();
    assert_eq!(child.state(), FiberState::Pending);
    enabled.store(true, Ordering::SeqCst);
    provision.notify();
    host.finish(child.settled()).unwrap();
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    enabled.store(false, Ordering::SeqCst);
    provision.notify();
    host.finish(child.settled()).unwrap();
    assert_eq!(child.state(), FiberState::Pending);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn internal_service_panic_preserves_provision_and_withdrawal() {
    for panic_during_provide in [true, false] {
        let host = TestExecutor::new();
        let root = Context::new(host.clone());
        let enabled = Arc::new(AtomicBool::new(panic_during_provide));
        let flag = enabled.clone();
        root.on::<cordis::InternalService>(move |_| {
            assert!(!flag.load(Ordering::SeqCst), "service notification failed");
            None
        })
        .unwrap();
        let child = root
            .inject(Inject::new().with::<Foo>(), |_| async { Ok(()) })
            .unwrap();
        let provision = root.provide::<Foo>(Arc::new(7)).unwrap();
        host.finish(child.settled()).unwrap();
        assert_eq!(child.state(), FiberState::Active);
        enabled.store(true, Ordering::SeqCst);
        host.finish(provision.dispose()).unwrap();
        assert_eq!(child.state(), FiberState::Pending);
        assert_eq!(root.service_count(), 0);
        assert!(
            root.errors()
                .iter()
                .any(|error| error.message.contains("service notification failed"))
        );
        host.finish(root.fiber().dispose()).unwrap();
    }
}
