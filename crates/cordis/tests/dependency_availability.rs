use cordis::{AnyArc, Context, Epoch, FiberState, Inject};
use cordis_testkit::TestExecutor;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Interface {
    minor: u32,
    value: usize,
}

fn dependency(required: bool, minimum: u32) -> Inject {
    Inject::new().with_named_options(
        "storage@1",
        required,
        Some(Arc::new(move |value| {
            value
                .downcast_ref::<Interface>()
                .is_some_and(|value| value.minor >= minimum)
        })),
    )
}

// ADR-0016：可选项缺席、出现和撤回均经过真实纪元与加载路径。
#[test]
fn optional_dependency_tracks_present_and_absent_epochs() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let seen = values.clone();
    let consumer = root
        .inject(dependency(false, 1), move |ctx| {
            let seen = seen.clone();
            async move {
                let value = ctx
                    .get_named("storage@1")
                    .ok()
                    .map(|value| value.downcast_ref::<Interface>().unwrap().value);
                seen.lock().unwrap().push(value);
                Ok(())
            }
        })
        .unwrap();
    host.finish(consumer.settled()).unwrap();
    assert_eq!(consumer.state(), FiberState::Active);
    assert!(matches!(consumer.epoch(), Epoch::Active { ref impls, .. } if impls == &[0]));
    let service = root
        .provide_named(
            "storage@1",
            Arc::new(Interface {
                minor: 1,
                value: 17,
            }),
        )
        .unwrap();
    host.finish(consumer.settled()).unwrap();
    host.finish(service.dispose()).unwrap();
    host.finish(consumer.settled()).unwrap();
    assert_eq!(*values.lock().unwrap(), [None, Some(17), None]);
    assert_eq!(consumer.state(), FiberState::Active);
    host.finish(root.fiber().dispose()).unwrap();
}

// ADR-0016、T-COE-6：可选接口存在期间，提供方等待已提交使用方完成清理。
#[test]
fn optional_consumer_releases_before_provider() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let order = Arc::new(Mutex::new(Vec::new()));
    let released = order.clone();
    let provider = root
        .inject(Inject::new(), move |ctx| {
            let released = released.clone();
            async move {
                ctx.effect("provider", move |fx| {
                    fx.defer(move || {
                        released.lock().unwrap().push("provider");
                        Ok(())
                    })
                })?;
                ctx.provide_named(
                    "storage@1",
                    Arc::new(Interface {
                        minor: 1,
                        value: 17,
                    }),
                )?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(provider.settled()).unwrap();
    let released = order.clone();
    let consumer = root
        .inject(dependency(false, 1), move |ctx| {
            let released = released.clone();
            async move {
                if let Ok(value) = ctx.get_named("storage@1") {
                    assert_eq!(value.downcast_ref::<Interface>().unwrap().value, 17);
                    let cleanup = ctx.clone();
                    ctx.effect("consumer", move |fx| {
                        fx.defer(move || {
                            assert_eq!(
                                cleanup
                                    .get_named("storage@1")?
                                    .downcast_ref::<Interface>()
                                    .unwrap()
                                    .value,
                                17
                            );
                            released.lock().unwrap().push("consumer");
                            Ok(())
                        })
                    })?;
                }
                Ok(())
            }
        })
        .unwrap();
    host.finish(consumer.settled()).unwrap();
    host.finish(provider.dispose()).unwrap();
    host.finish(consumer.settled()).unwrap();
    assert_eq!(*order.lock().unwrap(), ["consumer", "provider"]);
    assert_eq!(consumer.state(), FiberState::Active);
    host.finish(root.fiber().dispose()).unwrap();
}

// ADR-0016：同一实际绑定按每个使用方声明的最低次版本分别决定满足性。
#[test]
fn consumers_apply_distinct_minor_requirements() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let first = root
        .inject(dependency(true, 1), |_| async { Ok(()) })
        .unwrap();
    let second = root
        .inject(dependency(true, 2), |_| async { Ok(()) })
        .unwrap();
    let old = root
        .provide_named("storage@1", Arc::new(Interface { minor: 1, value: 1 }))
        .unwrap();
    host.finish(first.settled()).unwrap();
    host.finish(second.settled()).unwrap();
    assert_eq!(first.state(), FiberState::Active);
    assert_eq!(second.state(), FiberState::Pending);
    host.finish(old.dispose()).unwrap();
    root.provide_named("storage@1", Arc::new(Interface { minor: 2, value: 2 }))
        .unwrap();
    host.finish(first.settled()).unwrap();
    host.finish(second.settled()).unwrap();
    assert_eq!(first.state(), FiberState::Active);
    assert_eq!(second.state(), FiberState::Active);
    assert_eq!(first.epoch(), second.epoch());
    host.finish(root.fiber().dispose()).unwrap();
}

// ADR-0016、A-3：声明谓词读取真实绑定并重入内核，panic转为该声明的缺席结果。
#[test]
fn dependency_predicate_runs_outside_kernel_lock() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide_named(
        "storage@1",
        Arc::new(Interface {
            minor: 1,
            value: 17,
        }),
    )
    .unwrap();
    let check = root.clone();
    let consumer = root
        .inject(
            Inject::new().with_named_options(
                "storage@1",
                true,
                Some(Arc::new(move |value: &AnyArc| {
                    assert_eq!(check.service_count(), 1);
                    value.downcast_ref::<Interface>().unwrap().minor >= 1
                })),
            ),
            |_| async { Ok(()) },
        )
        .unwrap();
    host.finish(consumer.settled()).unwrap();
    assert_eq!(consumer.state(), FiberState::Active);
    let failed = root
        .inject(
            Inject::new().with_named_options(
                "storage@1",
                true,
                Some(Arc::new(|_| panic!("availability failure"))),
            ),
            |_| async { Ok(()) },
        )
        .unwrap();
    host.finish(failed.settled()).unwrap();
    assert_eq!(failed.state(), FiberState::Pending);
    assert!(
        root.errors()
            .iter()
            .any(|error| error.message.contains("availability failure"))
    );
    host.finish(root.fiber().dispose()).unwrap();
}

// ADR-0016、K-4：终点的活动实例保护保留，实例清空后显式退役释放闭包。
#[test]
fn forget_retires_only_empty_canonical_definition() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = cordis::ComponentDef::new("old", Inject::new(), |_, ()| async { Ok(()) });
    let resource = Arc::new(());
    let weak = Arc::downgrade(&resource);
    let next = cordis::ComponentDef::new("next", Inject::new(), move |_, ()| {
        let resource = resource.clone();
        async move {
            drop(resource);
            Ok(())
        }
    });
    root.registry().replace(&old, &next);
    drop(next);
    let instance = root.mount_with(&old, ()).unwrap();
    host.finish(instance.settled()).unwrap();
    assert!(root.registry().forget(&old).is_err());
    assert!(weak.upgrade().is_some());
    host.finish(instance.dispose()).unwrap();
    root.registry().forget(&old).unwrap();
    assert!(weak.upgrade().is_none());
    let restored = root.mount_with(&old, ()).unwrap();
    host.finish(restored.settled()).unwrap();
    assert_eq!(restored.name(), "old");
    host.finish(root.fiber().dispose()).unwrap();
}
