use cordis::{ComponentDef, Context, Epoch, FiberState, Inject};
use cordis_testkit::TestExecutor;
use std::time::Duration;

#[test]
fn current_epoch_observes_alive_running_and_expected_together() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let definition = ComponentDef::new("epoch-owner", Inject::new(), |ctx, ()| async move {
        ctx.sleep(Duration::from_millis(10)).await;
        Ok(())
    });
    let fiber = root.mount_with(&definition, ()).unwrap();
    let first = fiber.epoch();
    assert!(fiber.is_current_epoch(&first));
    assert!(!fiber.is_current_epoch(&Epoch::Inactive));
    host.run_until_stalled();

    let replacement = fiber.restart();
    let second = fiber.epoch();
    assert_ne!(first, second);
    // The target already names the replacement, while the in-flight body is old.
    assert!(!fiber.is_current_epoch(&first));
    assert!(!fiber.is_current_epoch(&second));
    host.advance(Duration::from_millis(10));
    host.run_until_stalled();
    assert_eq!(fiber.state(), FiberState::Loading);
    assert!(!fiber.is_current_epoch(&first));
    assert!(fiber.is_current_epoch(&second));
    host.advance(Duration::from_millis(10));
    host.finish(replacement).unwrap();
    assert!(fiber.is_current_epoch(&second));

    host.finish(fiber.dispose()).unwrap();
    assert!(!fiber.is_current_epoch(&second));
    assert!(!fiber.is_current_epoch(&fiber.epoch()));
}

#[test]
fn current_epoch_compares_every_dependency_identity() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide_named("first", std::sync::Arc::new(1usize))
        .unwrap();
    root.provide_named("second", std::sync::Arc::new(2usize))
        .unwrap();
    let definition = ComponentDef::new(
        "two-bindings",
        Inject::new().with_named("first").with_named("second"),
        |_, ()| async { Ok(()) },
    );
    let fiber = root.mount_with(&definition, ()).unwrap();
    host.finish(fiber.settled()).unwrap();
    let current = fiber.epoch();
    assert!(fiber.is_current_epoch(&current));
    let Epoch::Active { revision, impls } = current else {
        panic!("active epoch required")
    };
    assert_eq!(impls.len(), 2);
    for index in 0..impls.len() {
        let mut different = impls.clone();
        different[index] = different[index].wrapping_add(1);
        assert!(!fiber.is_current_epoch(&Epoch::Active {
            revision,
            impls: different
        }));
    }
    assert!(!fiber.is_current_epoch(&Epoch::Active {
        revision: revision + 1,
        impls
    }));
    host.finish(root.fiber().dispose()).unwrap();
}
