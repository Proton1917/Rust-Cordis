use cordis::{Context, Inject, Scoped, ServiceKey};
use cordis_testkit::TestExecutor;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Foo;
impl ServiceKey for Foo {
    const NAME: &'static str = "foo";
    type Value = AtomicUsize;
    type Meta = ();
}
struct FooBar;
impl ServiceKey for FooBar {
    const NAME: &'static str = "foo.bar";
    type Value = AtomicUsize;
    type Meta = ();
}
struct Baz;
impl ServiceKey for Baz {
    const NAME: &'static str = "baz";
    type Value = ();
    type Meta = ();
}
struct Qux;
impl ServiceKey for Qux {
    const NAME: &'static str = "qux";
    type Value = ();
    type Meta = ();
}

trait Associated {
    fn bar(&self) -> cordis::Result<Scoped<FooBar>>;
}
impl Associated for Scoped<Foo> {
    fn bar(&self) -> cordis::Result<Scoped<FooBar>> {
        self.requester().get::<FooBar>()
    }
}

#[test]
fn asc01_associated_service_follows_binding_lifetime() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(AtomicUsize::new(1))).unwrap();
    let child = root
        .inject(Inject::new(), |ctx| async move {
            ctx.provide::<FooBar>(Arc::new(AtomicUsize::new(42)))?;
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    let foo = root.get::<Foo>().unwrap();
    assert_eq!(foo.load(Ordering::SeqCst), 1);
    assert_eq!(foo.bar().unwrap().load(Ordering::SeqCst), 42);
    host.finish(child.dispose()).unwrap();
    assert!(foo.bar().is_err());
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn asc02_associated_property_update_uses_typed_service() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(AtomicUsize::new(2))).unwrap();
    let bar = root
        .provide::<FooBar>(Arc::new(AtomicUsize::new(0)))
        .unwrap();
    bar.set(Arc::new(AtomicUsize::new(3))).unwrap();
    let child = root
        .inject(
            Inject::new().with::<Foo>().with::<FooBar>(),
            |ctx| async move {
                let foo = ctx.get::<Foo>()?;
                assert_eq!(foo.load(Ordering::SeqCst), 2);
                assert_eq!(foo.bar()?.load(Ordering::SeqCst), 3);
                assert_eq!(foo.bar()?.caller().fiber(), ctx.fiber());
                Ok(())
            },
        )
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

struct Session(Context);
impl Session {
    fn answer(&self) -> cordis::Result<usize> {
        Ok(self.0.get::<FooBar>()?.load(Ordering::SeqCst))
    }
    fn set_answer(&self, value: usize) -> cordis::Result {
        self.0.get::<FooBar>()?.store(value + 1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn asc03_associated_session_access_uses_declared_service() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(AtomicUsize::new(0))).unwrap();
    let parent = root
        .inject(Inject::new().with::<Foo>(), |ctx| async move {
            assert!(Session(ctx.clone()).answer().is_err());
            ctx.provide::<FooBar>(Arc::new(AtomicUsize::new(42)))?;
            ctx.inject(Inject::new().with::<FooBar>(), |ctx| async move {
                assert_eq!(Session(ctx).answer()?, 42);
                Ok(())
            })?;
            Ok(())
        })
        .unwrap();
    host.finish(parent.settled()).unwrap();
    host.run_until_stalled();
    assert!(root.errors().is_empty());
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn asc04_session_accessor_uses_service_value_contract() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let provider = root
        .inject(Inject::new(), |ctx| async move {
            ctx.provide::<FooBar>(Arc::new(AtomicUsize::new(0)))?;
            Ok(())
        })
        .unwrap();
    host.finish(provider.settled()).unwrap();
    let denied = root
        .inject(Inject::new(), |ctx| async move {
            assert!(Session(ctx).set_answer(100).is_err());
            Ok(())
        })
        .unwrap();
    let allowed = root
        .inject(Inject::new().with::<FooBar>(), |ctx| async move {
            let session = Session(ctx);
            assert_eq!(session.answer()?, 0);
            session.set_answer(100)?;
            assert_eq!(session.answer()?, 101);
            Ok(())
        })
        .unwrap();
    host.finish(denied.settled()).unwrap();
    host.finish(allowed.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn asc05_associated_dependency_uses_current_service_declaration() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let foo = root
        .inject(Inject::new(), |ctx| async move {
            ctx.provide::<Foo>(Arc::new(AtomicUsize::new(1)))?;
            Ok(())
        })
        .unwrap();
    let bar = root
        .inject(Inject::new(), |ctx| async move {
            ctx.provide::<FooBar>(Arc::new(AtomicUsize::new(42)))?;
            Ok(())
        })
        .unwrap();
    host.finish(foo.settled()).unwrap();
    host.finish(bar.settled()).unwrap();
    let baz = root
        .inject(
            Inject::new().with::<Foo>().with::<FooBar>(),
            |ctx| async move {
                ctx.provide::<Baz>(Arc::new(()))?;
                Ok(())
            },
        )
        .unwrap();
    let qux = root
        .inject(Inject::new().with::<Foo>(), |ctx| async move {
            ctx.provide::<Qux>(Arc::new(()))?;
            Ok(())
        })
        .unwrap();
    host.finish(baz.settled()).unwrap();
    host.finish(qux.settled()).unwrap();
    assert_eq!(
        root.get::<Baz>()
            .unwrap()
            .dependency::<Foo>()
            .unwrap()
            .bar()
            .unwrap()
            .load(Ordering::SeqCst),
        42
    );
    assert!(
        root.get::<Qux>()
            .unwrap()
            .dependency::<Foo>()
            .unwrap()
            .bar()
            .is_err()
    );
    let child = root
        .inject(
            Inject::new().with::<Baz>().with::<Qux>(),
            |ctx| async move {
                assert_eq!(
                    ctx.get::<Baz>()?
                        .dependency::<Foo>()?
                        .bar()?
                        .load(Ordering::SeqCst),
                    42
                );
                assert!(ctx.get::<Qux>()?.dependency::<Foo>()?.bar().is_err());
                Ok(())
            },
        )
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}
