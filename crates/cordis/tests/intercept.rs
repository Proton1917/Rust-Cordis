use cordis::{Context, Inject, Merge, Realm, ServiceKey};
use cordis_testkit::TestExecutor;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct Meta {
    values: BTreeMap<&'static str, usize>,
    tags: Vec<&'static str>,
}
impl Merge for Meta {
    fn merge(&self, next: &Self) -> Self {
        let mut result = self.clone();
        result.values.extend(next.values.clone());
        result.tags.extend_from_slice(&next.tags);
        result
    }
}
fn meta(values: &[(&'static str, usize)]) -> Meta {
    Meta {
        values: values.iter().copied().collect(),
        tags: Vec::new(),
    }
}
struct Foo;
impl ServiceKey for Foo {
    const NAME: &'static str = "foo";
    type Value = ();
    type Meta = Meta;
}
struct Bar;
impl ServiceKey for Bar {
    const NAME: &'static str = "bar";
    type Value = ();
    type Meta = ();
}

/// T-COE-9、T-COE-11、T-COE-12、T-COE-13。
#[test]
fn icp01_context_layers_merge_outer_to_inner() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(())).unwrap();
    let child = root
        .inject(Inject::new().with::<Foo>(), |_| async { Ok(()) })
        .unwrap();
    host.finish(child.settled()).unwrap();
    let epoch = child.epoch();
    let derived = child
        .context()
        .intercept::<Foo>(meta(&[("a", 1), ("b", 1)]))
        .intercept::<Foo>(meta(&[("b", 2)]));
    assert_eq!(
        derived.get::<Foo>().unwrap().meta(),
        meta(&[("a", 1), ("b", 2)])
    );
    assert_eq!(root.get::<Foo>().unwrap().meta(), Meta::default());
    assert_eq!(child.epoch(), epoch);
    host.finish(root.fiber().dispose()).unwrap();
}

/// T-COE-10：声明、外层、内层的优先级逐层递增。
#[test]
fn icp02_injection_metadata_precedes_context_interception() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(())).unwrap();
    let child = root
        .intercept::<Foo>(meta(&[("b", 2)]))
        .inject(
            Inject::new().with_meta::<Foo>(meta(&[("a", 1), ("b", 1)])),
            |ctx| async move {
                assert_eq!(
                    ctx.intercept::<Foo>(meta(&[("c", 3)])).get::<Foo>()?.meta(),
                    meta(&[("a", 1), ("b", 2), ("c", 3)])
                );
                Ok(())
            },
        )
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn icp03_nearest_declaration_supplies_metadata() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(())).unwrap();
    let results = Arc::new(Mutex::new(Vec::new()));
    let seen = results.clone();
    let parent = root
        .inject(
            Inject::new().with_meta::<Foo>(meta(&[("a", 1)])),
            move |ctx| {
                let first = seen.clone();
                let second = seen.clone();
                async move {
                    ctx.inject(Inject::new(), move |ctx| {
                        let first = first.clone();
                        async move {
                            first.lock().unwrap().push(ctx.get::<Foo>()?.meta());
                            Ok(())
                        }
                    })?;
                    ctx.inject(
                        Inject::new().with_meta::<Foo>(meta(&[("b", 2)])),
                        move |ctx| {
                            let second = second.clone();
                            async move {
                                second.lock().unwrap().push(ctx.get::<Foo>()?.meta());
                                Ok(())
                            }
                        },
                    )?;
                    Ok(())
                }
            },
        )
        .unwrap();
    host.finish(parent.settled()).unwrap();
    host.run_until_stalled();
    assert_eq!(
        *results.lock().unwrap(),
        [meta(&[("a", 1)]), meta(&[("b", 2)])]
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn icp04_isolation_stops_inherited_injection_metadata() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let realm = Realm::new();
    root.provide::<Foo>(Arc::new(())).unwrap();
    root.isolate::<Foo>(realm)
        .provide::<Foo>(Arc::new(()))
        .unwrap();
    let child = root
        .inject(
            Inject::new().with_meta::<Foo>(meta(&[("a", 1)])),
            move |ctx| async move {
                assert_eq!(
                    ctx.isolate::<Foo>(realm).lookup::<Foo>().unwrap().meta(),
                    Meta::default()
                );
                Ok(())
            },
        )
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn icp05_own_provision_uses_its_own_intercepts() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(())).unwrap();
    let child = root
        .inject(
            Inject::new().with_meta::<Foo>(meta(&[("a", 1)])),
            |ctx| async move {
                let inner = ctx.isolate::<Foo>(Realm::new());
                inner.provide::<Foo>(Arc::new(()))?;
                assert_eq!(ctx.get::<Foo>()?.meta(), meta(&[("a", 1)]));
                assert_eq!(inner.get::<Foo>()?.meta(), Meta::default());
                assert_eq!(
                    inner
                        .intercept::<Foo>(meta(&[("b", 2)]))
                        .get::<Foo>()?
                        .meta(),
                    meta(&[("b", 2)])
                );
                Ok(())
            },
        )
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn icp06_nested_dependency_uses_definition_metadata_and_caller_intercepts() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(())).unwrap();
    let provider = root
        .inject(
            Inject::new().with_meta::<Foo>(meta(&[("a", 2)])),
            |ctx| async move {
                ctx.provide::<Bar>(Arc::new(()))?;
                Ok(())
            },
        )
        .unwrap();
    host.finish(provider.settled()).unwrap();
    let service = root
        .intercept::<Foo>(meta(&[("b", 1)]))
        .get::<Bar>()
        .unwrap();
    assert_eq!(
        service.dependency::<Foo>().unwrap().meta(),
        meta(&[("a", 2), ("b", 1)])
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn icp07_custom_metadata_merge_concatenates_tags() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Foo>(Arc::new(())).unwrap();
    let tags = |tag| Meta {
        values: BTreeMap::new(),
        tags: vec![tag],
    };
    let child = root
        .intercept::<Foo>(tags("outer"))
        .inject(
            Inject::new().with_meta::<Foo>(tags("inject")),
            |ctx| async move {
                let inner = Meta {
                    values: BTreeMap::new(),
                    tags: vec!["inner"],
                };
                assert_eq!(
                    ctx.intercept::<Foo>(inner).get::<Foo>()?.meta().tags,
                    ["inject", "outer", "inner"]
                );
                Ok(())
            },
        )
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}
