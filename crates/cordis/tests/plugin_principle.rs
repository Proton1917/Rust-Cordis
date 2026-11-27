//! PP-1–PP-5: the public component boundary owns extension, replacement and cleanup.
//!
//! These are finite executions of the real kernel. The source certificate and
//! arbitrary-trace theorems are checked separately by plugin_principle_probe.py.

use cordis::{ComponentDef, Context, FiberState, Inject, Merge, Realm, ServiceKey};
use cordis_testkit::TestExecutor;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Observation {
    Load(&'static str, usize),
    ReleaseConsumer(&'static str, usize),
    ReleaseProvider(&'static str, usize),
}

type Trace = Arc<Mutex<Vec<Observation>>>;

fn provider(
    name: &str,
    key: &str,
    label: &'static str,
    offset: usize,
    trace: Trace,
) -> ComponentDef {
    let key = key.to_owned();
    ComponentDef::new(name.to_owned(), Inject::new(), move |ctx, raw: usize| {
        let (key, trace) = (key.clone(), trace.clone());
        async move {
            let value = raw + offset;
            ctx.effect("provider resource", move |fx| {
                fx.defer(move || {
                    trace
                        .lock()
                        .unwrap()
                        .push(Observation::ReleaseProvider(label, value));
                    Ok(())
                })
            })?;
            ctx.provide_named(key, Arc::new(value))?;
            Ok(())
        }
    })
}

fn consumer(name: &str, key: &str, trace: Trace) -> ComponentDef {
    let key = key.to_owned();
    ComponentDef::new(
        name.to_owned(),
        Inject::new().with_named(key.clone()),
        move |ctx, label: &'static str| {
            let (key, trace) = (key.clone(), trace.clone());
            async move {
                let value = *ctx
                    .get_named(key.as_str())?
                    .downcast_ref::<usize>()
                    .unwrap();
                trace.lock().unwrap().push(Observation::Load(label, value));
                ctx.effect("consumer resource", move |fx| {
                    fx.defer(move || {
                        trace
                            .lock()
                            .unwrap()
                            .push(Observation::ReleaseConsumer(label, value));
                        Ok(())
                    })
                })?;
                Ok(())
            }
        },
    )
}

// PP-1–PP-4: names and configuration are data; dependency and owner determine
// activation. A replaced provider drains its consumer before its own resource.
#[test]
fn extensions_replace_through_declared_dependencies_and_owned_effects() {
    for name in [
        "",
        "fresh.extension/Ω",
        "cordis-supervisor",
        "web-owner-new",
    ] {
        for raw in [0usize, 1, 29] {
            let host = TestExecutor::new();
            let root = Context::new(host.clone());
            let owner = root.fiber();
            let trace = Trace::default();
            let key = format!("new.service/{raw}");
            let isolated = root.isolate_named(key.as_str(), Realm::new());
            let use_service = consumer(name, &key, trace.clone());
            let main = root.mount_with(&use_service, "main").unwrap();
            let sibling = isolated.mount_with(&use_service, "isolated").unwrap();
            host.run_until_stalled();
            assert_eq!(main.state(), FiberState::Pending);
            assert_eq!(sibling.state(), FiberState::Pending);
            assert!(trace.lock().unwrap().is_empty());

            let old = provider(name, &key, "main", 0, trace.clone());
            let next = provider(name, &key, "main", 11, trace.clone());
            let independent = provider(name, &key, "isolated", 97, trace.clone());
            assert_ne!(old.id(), next.id());
            assert_ne!(old.id(), independent.id());
            let main_provider = root.mount_with(&old, raw).unwrap();
            host.finish(main_provider.settled()).unwrap();
            host.finish(main.settled()).unwrap();
            assert_eq!(sibling.state(), FiberState::Pending);
            let isolated_provider = isolated.mount_with(&independent, raw).unwrap();
            host.finish(isolated_provider.settled()).unwrap();
            host.finish(sibling.settled()).unwrap();
            let isolated_epoch = sibling.epoch();
            let isolated_provider_epoch = isolated_provider.epoch();

            assert_eq!(
                root.registry().replace(&old, &next),
                [main_provider.clone()]
            );
            host.finish(main_provider.settled()).unwrap();
            host.finish(main.settled()).unwrap();
            assert_eq!(root.registry().fibers(&old), [main_provider.clone()]);
            assert_eq!(sibling.epoch(), isolated_epoch);
            assert_eq!(isolated_provider.epoch(), isolated_provider_epoch);
            assert_eq!(
                *trace.lock().unwrap(),
                [
                    Observation::Load("main", raw),
                    Observation::Load("isolated", raw + 97),
                    Observation::ReleaseConsumer("main", raw),
                    Observation::ReleaseProvider("main", raw),
                    Observation::Load("main", raw + 11),
                ]
            );

            host.finish(owner.dispose()).unwrap();
            let once = trace.lock().unwrap().clone();
            host.finish(owner.dispose()).unwrap();
            assert_eq!(*trace.lock().unwrap(), once);
            for (label, value) in [("main", raw), ("main", raw + 11), ("isolated", raw + 97)] {
                let releases = [
                    Observation::ReleaseConsumer(label, value),
                    Observation::ReleaseProvider(label, value),
                ];
                for release in releases {
                    assert_eq!(once.iter().filter(|entry| **entry == release).count(), 1);
                }
                let consumer = once
                    .iter()
                    .position(|entry| *entry == Observation::ReleaseConsumer(label, value))
                    .unwrap();
                let provider = once
                    .iter()
                    .position(|entry| *entry == Observation::ReleaseProvider(label, value))
                    .unwrap();
                assert!(consumer < provider);
            }
            assert!(root.registry().is_empty());
            assert!(root.errors().is_empty());
        }
    }
}

// PP-5: an application-defined composite creates independent instances using
// exactly the same mount boundary as its own parent, and owns their teardown.
#[test]
fn composed_extensions_keep_instance_identity_and_parent_ownership() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let child_events = events.clone();
    let child = ComponentDef::new("new child", Inject::new(), move |ctx, config: usize| {
        let events = child_events.clone();
        async move {
            events.lock().unwrap().push(("load", config));
            ctx.effect("child resource", move |fx| {
                fx.defer(move || {
                    events.lock().unwrap().push(("release", config));
                    Ok(())
                })
            })?;
            Ok(())
        }
    });
    let members = Arc::new(Mutex::new(Vec::new()));
    let registered = members.clone();
    let composite = ComponentDef::new("new composite", Inject::new(), move |ctx, base: usize| {
        let (child, members) = (child.clone(), registered.clone());
        async move {
            let first = ctx.mount_with(&child, base)?;
            first.settled().await?;
            let second = ctx.mount_with(&child, base + 1)?;
            second.settled().await?;
            members.lock().unwrap().extend([first, second]);
            Ok(())
        }
    });
    let parent = root.mount_with(&composite, 40usize).unwrap();
    host.finish(parent.settled()).unwrap();
    let independent = root.mount_with(&composite, 90usize).unwrap();
    host.finish(independent.settled()).unwrap();
    let children = members.lock().unwrap().clone();
    assert_eq!(children.len(), 4);
    assert_ne!(children[0], children[1]);
    host.finish(parent.dispose()).unwrap();
    assert_eq!(children[2].state(), FiberState::Active);
    assert_eq!(children[3].state(), FiberState::Active);
    assert_eq!(
        *events.lock().unwrap(),
        [
            ("load", 40),
            ("load", 41),
            ("load", 90),
            ("load", 91),
            ("release", 41),
            ("release", 40)
        ]
    );
    host.finish(root.fiber().dispose()).unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        [
            ("load", 40),
            ("load", 41),
            ("load", 90),
            ("load", 91),
            ("release", 41),
            ("release", 40),
            ("release", 91),
            ("release", 90)
        ]
    );
    assert!(root.registry().is_empty());
    assert!(root.errors().is_empty());
}

// Paper §3.1: recovery composes the actual registered partial inverses in the
// reverse order. Starting from 1, +2 then ×2 reaches 6 and recovers 1.
#[test]
fn registered_inverses_restore_context_state() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let value = Arc::new(Mutex::new(1i64));
    let first = value.clone();
    root.effect("add two", move |scope| {
        *first.lock().unwrap() += 2;
        scope.defer(move || {
            *first.lock().unwrap() -= 2;
            Ok(())
        })
    })
    .unwrap();
    let second = value.clone();
    root.effect("double", move |scope| {
        *second.lock().unwrap() *= 2;
        scope.defer(move || {
            let mut value = second.lock().unwrap();
            assert_eq!(*value % 2, 0);
            *value /= 2;
            Ok(())
        })
    })
    .unwrap();
    assert_eq!(*value.lock().unwrap(), 6);
    let owner = root.fiber();
    host.finish(owner.dispose()).unwrap();
    assert_eq!(*value.lock().unwrap(), 1);
    host.finish(owner.dispose()).unwrap();
    assert_eq!(*value.lock().unwrap(), 1);
    assert!(root.errors().is_empty());
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct Layers(Vec<&'static str>);
impl Merge for Layers {
    fn merge(&self, next: &Self) -> Self {
        Self(self.0.iter().chain(&next.0).copied().collect())
    }
}
struct ScopedValue;
impl ServiceKey for ScopedValue {
    const NAME: &'static str = "new.scoped.value";
    type Value = usize;
    type Meta = Layers;
}

// Paper §3.2 and Algorithms 2–3: metadata follows declaration/outer/inner order;
// disposing a captured registration changes only its originally selected realm.
#[test]
fn coeffect_views_preserve_parent_metadata_and_registration_realm() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<ScopedValue>(Arc::new(5)).unwrap();
    let parent_realm = root.realm::<ScopedValue>();
    let first = root.isolate::<ScopedValue>(Realm::new());
    let registration = first.provide::<ScopedValue>(Arc::new(11)).unwrap();
    let second = first.isolate::<ScopedValue>(Realm::new());
    second.provide::<ScopedValue>(Arc::new(23)).unwrap();
    assert_eq!(root.realm::<ScopedValue>(), parent_realm);
    assert_eq!(*root.get::<ScopedValue>().unwrap(), 5);
    assert_eq!(*first.get::<ScopedValue>().unwrap(), 11);
    assert_eq!(*second.get::<ScopedValue>().unwrap(), 23);

    let derived = root
        .intercept::<ScopedValue>(Layers(vec!["outer"]))
        .intercept::<ScopedValue>(Layers(vec!["inner"]));
    let observer = derived
        .inject(
            Inject::new().with_meta::<ScopedValue>(Layers(vec!["declaration"])),
            |ctx| async move {
                assert_eq!(
                    ctx.get::<ScopedValue>()?.meta(),
                    Layers(vec!["declaration", "outer", "inner"])
                );
                Ok(())
            },
        )
        .unwrap();
    host.finish(observer.settled()).unwrap();
    let epoch = observer.epoch();
    assert_eq!(root.get::<ScopedValue>().unwrap().meta(), Layers::default());
    host.finish(registration.dispose()).unwrap();
    assert!(first.get::<ScopedValue>().is_err());
    assert_eq!(*second.get::<ScopedValue>().unwrap(), 23);
    assert_eq!(*root.get::<ScopedValue>().unwrap(), 5);
    assert_eq!(observer.epoch(), epoch);
    host.finish(root.fiber().dispose()).unwrap();
    assert!(root.errors().is_empty());
}
