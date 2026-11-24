use cordis::{ComponentDef, Context, FiberState, Inject, Realm};
use cordis_testkit::TestExecutor;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn epoch_guards_follow_derived_views_and_cannot_be_rebound() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let owner = root.inject(Inject::new(), |_| async { Ok(()) }).unwrap();
    host.finish(owner.settled()).unwrap();
    let ordinary = owner.context();
    let captured = ordinary.for_epoch(Arc::new(owner.epoch()));
    let derived = captured
        .extend(7usize)
        .isolate_named("domain", Realm::new())
        .intercept_named("domain", Arc::new(()));
    derived.effect("current", |_| Ok(())).unwrap();
    host.finish(owner.restart()).unwrap();
    assert!(ordinary.is_current());
    assert!(!captured.is_current());
    assert!(!derived.is_current());
    let rebound = derived.for_epoch(Arc::new(owner.epoch()));
    assert!(
        !rebound.is_current(),
        "all guards in one fiber remain effective"
    );
    assert!(
        rebound
            .effect("old", |_| panic!("stale callback ran"))
            .is_err()
    );
    assert!(
        rebound
            .provide_named("old-service", Arc::new(1usize))
            .is_err()
    );
    assert!(rebound.on_named("old-event", |_| Ok(None)).is_err());
    assert!(rebound.inject(Inject::new(), |_| async { Ok(()) }).is_err());

    // S-FBR-3 keeps the existing live-fiber contract for an ordinary Context.
    ordinary.effect("new", |_| Ok(())).unwrap();
    let pending = root
        .inject(Inject::new().with_named("missing"), |_| async { Ok(()) })
        .unwrap();
    host.finish(pending.settled()).unwrap();
    assert_eq!(pending.state(), FiberState::Pending);
    pending.context().effect("pending", |_| Ok(())).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn child_scope_owns_its_effects_under_its_own_epoch() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let owner = root.inject(Inject::new(), |_| async { Ok(()) }).unwrap();
    host.finish(owner.settled()).unwrap();
    host.finish(owner.restart()).unwrap();
    let parent_epoch = Arc::new(owner.epoch());
    let captured = owner.context().for_epoch(parent_epoch.clone());
    let cleanups = Arc::new(AtomicUsize::new(0));
    let observed = cleanups.clone();
    let definition = ComponentDef::new("child", Inject::new(), move |context, ()| {
        let observed = observed.clone();
        async move {
            assert!(context.is_current());
            let context = context.for_epoch(Arc::new(context.fiber().epoch()));
            context.effect("child-resource", move |fx| {
                fx.defer(move || {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })?;
            Ok(())
        }
    });
    let child = captured.mount_with(&definition, ()).unwrap();
    host.finish(child.settled()).unwrap();
    assert_ne!(*parent_epoch, child.epoch());
    assert_eq!(child.state(), FiberState::Active);
    host.finish(owner.restart()).unwrap();
    assert_eq!(child.state(), FiberState::Disposed);
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn retired_view_reads_its_committed_service_during_cleanup() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let service = root.provide_named("resource", Arc::new(41usize)).unwrap();
    let cleaned = Arc::new(Mutex::new(Vec::new()));
    let observed = cleaned.clone();
    let owner = root
        .inject(Inject::new().with_named("resource"), move |context| {
            let observed = observed.clone();
            async move {
                let context = context.for_epoch(Arc::new(context.fiber().epoch()));
                let cleanup = context.clone();
                context.effect("resource-user", move |fx| {
                    fx.defer(move || {
                        assert!(!cleanup.is_current());
                        let value = *cleanup
                            .get_named("resource")?
                            .downcast_ref::<usize>()
                            .unwrap();
                        assert!(cleanup.effect("late", |_| Ok(())).is_err());
                        observed.lock().unwrap().push(value);
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(owner.settled()).unwrap();
    host.finish(service.dispose()).unwrap();
    assert_eq!(owner.state(), FiberState::Pending);
    assert_eq!(*cleaned.lock().unwrap(), [41]);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn admitted_effect_finishes_its_cleanup_handoff_before_retirement() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let owner = root.inject(Inject::new(), |_| async { Ok(()) }).unwrap();
    host.finish(owner.settled()).unwrap();
    let captured = owner.context().for_epoch(Arc::new(owner.epoch()));
    let cleanups = Arc::new(AtomicUsize::new(0));
    let observed = cleanups.clone();
    let retiring = owner.clone();
    let clock = host.clone();
    let effect = captured
        .effect("admitted", move |fx| {
            drop(retiring.restart());
            clock.run_until_stalled();
            assert!(
                !retiring.is_settled(),
                "retirement waits for the admitted body"
            );
            assert!(!fx.is_current());
            fx.defer(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })?;
            clock.run_until_stalled();
            assert!(!retiring.is_settled());
            Ok(())
        })
        .unwrap();
    host.finish(owner.settled()).unwrap();
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
    host.finish(effect.dispose()).unwrap();
    assert_eq!(cleanups.load(Ordering::SeqCst), 1);
    host.finish(root.fiber().dispose()).unwrap();
}
