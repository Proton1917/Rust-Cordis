use cordis::{ComponentDef, Context, Inject, Realm, Scoped, ServiceKey};
use cordis_testkit::TestExecutor;
use std::sync::Arc;

struct Inner;
impl ServiceKey for Inner {
    const NAME: &'static str = "inner";
    type Value = &'static str;
    type Meta = ();
}
struct Outer;
impl ServiceKey for Outer {
    const NAME: &'static str = "outer";
    type Value = &'static str;
    type Meta = ();
}
struct Loader;
impl ServiceKey for Loader {
    const NAME: &'static str = "loader";
    type Value = ();
    type Meta = ();
}

fn setup(declare: bool) -> (Arc<TestExecutor>, Context, cordis::Fiber, cordis::Fiber) {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let inner = root
        .inject(Inject::new(), |ctx| async move {
            ctx.provide::<Inner>(Arc::new("hello"))?;
            Ok(())
        })
        .unwrap();
    let inject = if declare {
        Inject::new().with::<Inner>()
    } else {
        Inject::new()
    };
    let outer = root
        .inject(inject, |ctx| async move {
            ctx.provide::<Outer>(Arc::new("outer"))?;
            Ok(())
        })
        .unwrap();
    host.run_until_stalled();
    host.finish(outer.settled()).unwrap();
    (host, root, inner, outer)
}

#[test]
fn shd01_direct_caller_and_service_definition_stay_separate() {
    let (host, root, inner, outer) = setup(true);
    let service = root.get::<Outer>().unwrap();
    let dependency = service.dependency::<Inner>().unwrap();
    assert_eq!(dependency.requester().fiber(), outer);
    assert_eq!(dependency.caller().fiber(), root.fiber());
    assert_eq!(dependency.definition().fiber(), inner);
    assert_eq!(service.definition().fiber(), outer);
    let (inner_id, outer_id) = (inner.clone(), outer.clone());
    let child = root
        .inject(Inject::new().with::<Outer>(), move |ctx| {
            let (inner_id, outer_id) = (inner_id.clone(), outer_id.clone());
            async move {
                let dependency = ctx.get::<Outer>()?.dependency::<Inner>()?;
                assert_eq!(dependency.requester().fiber(), outer_id);
                assert_eq!(dependency.caller().fiber(), ctx.fiber());
                assert_eq!(dependency.definition().fiber(), inner_id);
                Ok(())
            }
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn shd02_root_entry_enforces_service_dependency_declaration() {
    let (host, root, _, _) = setup(false);
    assert_eq!(
        root.get::<Outer>()
            .unwrap()
            .dependency::<Inner>()
            .unwrap_err()
            .message
            .as_ref(),
        "cannot get property \"inner\" without inject"
    );
    let child = root
        .inject(Inject::new().with::<Outer>(), |ctx| async move {
            assert!(ctx.get::<Outer>()?.dependency::<Inner>().is_err());
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

struct Holder(Context);
impl Holder {
    fn probe(&self) -> cordis::Result<(&'static str, &'static str)> {
        Ok((*self.0.get::<Outer>()?, *self.0.get::<Inner>()?))
    }
}
fn holder(service: &Scoped<Outer>) -> Holder {
    Holder(service.definition().clone())
}

#[test]
fn shd03_captured_definition_resolves_self_and_dependencies() {
    let (host, root, _, outer) = setup(true);
    let captured = holder(&root.get::<Outer>().unwrap());
    assert_eq!(captured.probe().unwrap(), ("outer", "hello"));
    assert_eq!(captured.0.fiber(), outer);
    assert_eq!(
        root.logger()
            .buffer()
            .iter()
            .filter(|message| message.level == cordis::LogLevel::Warn)
            .count(),
        0
    );
    let child = root
        .inject(Inject::new().with::<Outer>(), |ctx| async move {
            assert_eq!(holder(&ctx.get::<Outer>()?).probe()?, ("outer", "hello"));
            assert_eq!(
                ctx.logger()
                    .buffer()
                    .iter()
                    .filter(|message| message.level == cordis::LogLevel::Warn)
                    .count(),
                0
            );
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(child.dispose()).unwrap();
    assert_eq!(captured.probe().unwrap(), ("outer", "hello"));
    assert_eq!(
        root.logger()
            .buffer()
            .iter()
            .filter(|message| message.level == cordis::LogLevel::Warn)
            .count(),
        0
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn shd04_definition_binding_and_use_site_realm_are_independent() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let (first, second) = (Realm::new(), Realm::new());
    root.isolate::<Inner>(first)
        .provide::<Inner>(Arc::new("first"))
        .unwrap();
    root.isolate::<Inner>(second)
        .provide::<Inner>(Arc::new("second"))
        .unwrap();
    let relay = root
        .isolate::<Inner>(first)
        .inject(Inject::new().with::<Inner>(), |ctx| async move {
            ctx.provide::<Outer>(Arc::new("relay"))?;
            Ok(())
        })
        .unwrap();
    host.finish(relay.settled()).unwrap();
    let caller = root.isolate::<Inner>(second);
    let reached = caller
        .get::<Outer>()
        .unwrap()
        .dependency::<Inner>()
        .unwrap();
    assert_eq!(*reached, "first");
    assert_eq!(reached.definition().realm::<Inner>(), first);
    assert_eq!(reached.caller().realm::<Inner>(), second);
    assert_eq!(*caller.get::<Inner>().unwrap(), "second");
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn shd05_root_defined_service_keeps_root_access() {
    let (host, root, _, _) = setup(false);
    root.provide::<Loader>(Arc::new(())).unwrap();
    assert_eq!(
        *root.get::<Loader>().unwrap().dependency::<Inner>().unwrap(),
        "hello"
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn shd06_plain_service_receives_explicit_caller() {
    let (host, root, _, outer) = setup(true);
    root.provide::<Loader>(Arc::new(())).unwrap();
    let direct = outer.context().lookup::<Loader>().unwrap();
    assert_eq!(direct.caller().fiber(), outer);
    assert_eq!(
        direct.caller().get::<Inner>().unwrap().value().as_ref(),
        &"hello"
    );
    host.finish(root.fiber().dispose()).unwrap();
}

fn invoke(service: &Scoped<Inner>) -> Context {
    service.requester().clone()
}

#[test]
fn shd07_callable_service_receives_direct_service_caller() {
    let (host, root, _, outer) = setup(true);
    let dependency = root.get::<Outer>().unwrap().dependency::<Inner>().unwrap();
    assert_eq!(invoke(&dependency).fiber(), outer);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn shd08_service_created_components_use_callers_lifecycle() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Loader>(Arc::new(())).unwrap();
    let owner_def = ComponentDef::new(
        "owner",
        Inject::new().with::<Loader>(),
        |ctx, ()| async move {
            let loader = ctx.get::<Loader>()?;
            loader.caller().inject(Inject::new(), |ctx| async move {
                ctx.provide::<Inner>(Arc::new("server"))?;
                Ok(())
            })?;
            loader
                .caller()
                .inject(Inject::new().with::<Inner>(), |ctx| async move {
                    assert_eq!(*ctx.get::<Inner>()?, "server");
                    Ok(())
                })?;
            Ok(())
        },
    );
    let owner = root.mount_with(&owner_def, ()).unwrap();
    host.finish(owner.settled()).unwrap();
    host.run_until_stalled();
    assert!(root.errors().is_empty());
    assert_eq!(root.registry().len(), 3);
    host.finish(owner.dispose()).unwrap();
    assert!(root.lookup::<Inner>().is_none());
    assert_eq!(root.registry().len(), 0);
    host.finish(root.fiber().dispose()).unwrap();
}
