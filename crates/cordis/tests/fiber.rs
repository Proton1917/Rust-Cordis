use cordis::{
    ComponentDef, Context, Error, Event, FiberState, Inject, InternalStatus, InternalUpdate,
    RawConfig, SyncKind, ValidationError, ValidationIssue,
};
use cordis_testkit::TestExecutor;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Tick;
impl Event for Tick {
    const NAME: &'static str = "tick";
    type Args = ();
    type Output = ();
    type Kind = SyncKind;
}

// FBR-04、T-LIF-4、T-LIF-5：加载失败回收已登记效应，同组件的独立实例继续服务。
#[test]
fn component_failure_cleans_partial_effects() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let def = ComponentDef::new("failing", Inject::new(), move |ctx, valid: bool| {
        let calls = calls.clone();
        async move {
            ctx.on::<Tick>(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                None
            })?;
            if valid {
                Ok(())
            } else {
                Err(Error::new("component error"))
            }
        }
    });
    let failed = root.mount_with(&def, false).unwrap();
    let active = root.mount_with(&def, true).unwrap();
    assert_eq!(
        host.finish(failed.settled()).unwrap_err().message.as_ref(),
        "component error"
    );
    host.finish(active.settled()).unwrap();
    assert_eq!(failed.state(), FiberState::Failed);
    assert_eq!(active.state(), FiberState::Active);
    root.emit::<Tick>(&());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(root.errors().len(), 1);
    host.finish(root.fiber().dispose()).unwrap();
}

// FBR-07：更新等待恢复任务完成。
#[test]
fn update_waits_for_asynchronous_recovery() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let def = ComponentDef::new("recover", Inject::new(), |ctx, valid: bool| async move {
        if !valid {
            return Err(Error::new("boom"));
        }
        ctx.sleep(Duration::from_millis(10)).await;
        Ok(())
    });
    let fiber = root.mount_with(&def, false).unwrap();
    assert!(host.finish(fiber.settled()).is_err());
    let recovery = fiber.update(true);
    host.run_until_stalled();
    assert_eq!(fiber.state(), FiberState::Loading);
    assert!(!fiber.is_settled());
    host.advance(Duration::from_millis(10));
    host.finish(recovery).unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    host.finish(fiber.dispose()).unwrap();
}

// FBR-08：恢复错误交给更新等待者。
#[test]
fn update_returns_failed_recovery() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let def = ComponentDef::new("failure", Inject::new(), |_, message: String| async move {
        Err(Error::new(message))
    });
    let fiber = root.mount_with(&def, "initial failure".to_owned()).unwrap();
    assert_eq!(
        host.finish(fiber.settled()).unwrap_err().message.as_ref(),
        "initial failure"
    );
    assert_eq!(
        host.finish(fiber.update("retry failure".to_owned()))
            .unwrap_err()
            .message
            .as_ref(),
        "retry failure"
    );
    assert_eq!(fiber.state(), FiberState::Failed);
    host.finish(fiber.dispose()).unwrap();
}

fn failing_update(root: &Context) -> cordis::Fiber {
    let def = ComponentDef::new("update", Inject::new(), |_, fail: bool| async move {
        if fail {
            Err(Error::new("boom"))
        } else {
            Ok(())
        }
    });
    root.mount_with(&def, false).unwrap()
}

// FBR-09：活跃实例重载的错误交给更新等待者。
#[test]
fn update_returns_failed_reload() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let fiber = failing_update(&root);
    host.finish(fiber.settled()).unwrap();
    assert_eq!(
        host.finish(fiber.update(true))
            .unwrap_err()
            .message
            .as_ref(),
        "boom"
    );
    assert_eq!(fiber.state(), FiberState::Failed);
    host.finish(fiber.dispose()).unwrap();
}

// FBR-10：更新的执行由内核持有，调用方释放等待句柄后仍记录失败。
#[test]
fn dropped_update_waiter_preserves_failure_state() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let fiber = failing_update(&root);
    host.finish(fiber.settled()).unwrap();
    drop(fiber.update(true));
    host.run_until_stalled();
    assert_eq!(fiber.state(), FiberState::Failed);
    assert_eq!(root.errors().last().unwrap().message.as_ref(), "boom");
    host.finish(fiber.dispose()).unwrap();
}

// FBR-11：单项清理失败后其余效应继续释放，释放操作正常稳定。
#[test]
fn disposal_records_error_and_releases_other_effects() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let released = Arc::new(Mutex::new(Vec::new()));
    let events = released.clone();
    let fiber = root
        .inject(Inject::new(), move |ctx| {
            let events = events.clone();
            async move {
                let before = events.clone();
                ctx.effect("success", |fx| {
                    fx.defer(move || {
                        before.lock().unwrap().push("success");
                        Ok(())
                    })
                })?;
                ctx.effect("failure", |fx| {
                    fx.defer(move || {
                        events.lock().unwrap().push("failure");
                        Err(Error::new("test"))
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(fiber.settled()).unwrap();
    host.finish(fiber.dispose()).unwrap();
    assert_eq!(*released.lock().unwrap(), ["failure", "success"]);
    assert_eq!(root.errors().last().unwrap().message.as_ref(), "test");
}

fn collect_transitions(
    root: &Context,
    fiber: &cordis::Fiber,
) -> Arc<Mutex<Vec<(FiberState, FiberState)>>> {
    let changes = Arc::new(Mutex::new(Vec::new()));
    let received = changes.clone();
    let expected = fiber.clone();
    root.on::<InternalStatus>(move |(current, old)| {
        if current == &expected {
            received.lock().unwrap().push((*old, current.state()));
        }
        None
    })
    .unwrap();
    changes
}

// FBR-13：失败实例释放时直接进入 Disposed。
#[test]
fn failed_fiber_disposes_with_one_status_transition() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let fiber = root
        .inject(Inject::new(), |_| async { Err(Error::new("boom")) })
        .unwrap();
    assert!(host.finish(fiber.settled()).is_err());
    let changes = collect_transitions(&root, &fiber);
    host.finish(fiber.dispose()).unwrap();
    assert_eq!(
        *changes.lock().unwrap(),
        [(FiberState::Failed, FiberState::Disposed)]
    );
    assert!(!fiber.is_alive());
    assert!(fiber.is_settled());
    host.finish(root.fiber().dispose()).unwrap();
}

// FBR-14：活跃实例释放时经过 Unloading。
#[test]
fn active_fiber_disposal_status_sequence() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let fiber = root.inject(Inject::new(), |_| async { Ok(()) }).unwrap();
    host.finish(fiber.settled()).unwrap();
    let changes = collect_transitions(&root, &fiber);
    host.finish(fiber.dispose()).unwrap();
    assert_eq!(
        *changes.lock().unwrap(),
        [
            (FiberState::Active, FiberState::Unloading),
            (FiberState::Unloading, FiberState::Disposed)
        ]
    );
    assert!(!fiber.is_alive());
    assert!(fiber.is_settled());
    host.finish(root.fiber().dispose()).unwrap();
}

// FBR-15：克隆句柄观察同一个配置和生命周期。
#[test]
fn cloned_handle_updates_shared_configuration() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let received = values.clone();
    let def = ComponentDef::new("clone", Inject::new(), move |_, value: String| {
        received.lock().unwrap().push(value);
        async { Ok(()) }
    });
    let fiber = root.mount_with(&def, "hello".to_owned()).unwrap();
    let clone = fiber.clone();
    host.finish(fiber.settled()).unwrap();
    host.finish(clone.update("world".to_owned())).unwrap();
    host.finish(clone.update("!!!".to_owned())).unwrap();
    assert_eq!(*values.lock().unwrap(), ["hello", "world", "!!!"]);
    assert_eq!(fiber.config::<String>().as_deref().unwrap(), "!!!");
    assert_eq!(fiber.state(), FiberState::Active);
    host.finish(fiber.dispose()).unwrap();
}

// FBR-16、T-LIF-2：重启推进修订号，克隆句柄共享结果。
#[test]
fn cloned_handle_restarts_same_fiber() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let received = values.clone();
    let fiber = root
        .inject(Inject::new(), move |ctx| {
            received.lock().unwrap().push(ctx.fiber().epoch());
            async { Ok(()) }
        })
        .unwrap();
    host.finish(fiber.settled()).unwrap();
    host.finish(fiber.clone().restart()).unwrap();
    let epochs = values.lock().unwrap();
    assert_eq!(epochs.len(), 2);
    assert_ne!(epochs[0], epochs[1]);
    assert_eq!(fiber.state(), FiberState::Active);
    drop(epochs);
    host.finish(fiber.dispose()).unwrap();
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Source {
    ok: bool,
    extra: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Resolved {
    source: Source,
    resolved: bool,
}
fn validate(source: &Source) -> std::result::Result<Resolved, ValidationError> {
    if source.ok {
        Ok(Resolved {
            source: source.clone(),
            resolved: true,
        })
    } else {
        Err(ValidationError {
            issues: vec![ValidationIssue {
                message: "not ok".into(),
                path: Vec::new(),
            }],
        })
    }
}

// FBR-18：首次加载校验失败时保持组件执行资源为空。
#[test]
fn invalid_initial_configuration_fails_before_apply() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let received = values.clone();
    let def = ComponentDef::validated("validated", Inject::new(), validate, move |_, value| {
        received.lock().unwrap().push(value);
        async { Ok(()) }
    });
    let fiber = root
        .mount_with(
            &def,
            Source {
                ok: false,
                extra: 0,
            },
        )
        .unwrap();
    assert!(host.finish(fiber.settled()).is_err());
    assert_eq!(fiber.state(), FiberState::Failed);
    assert!(values.lock().unwrap().is_empty());
    host.finish(fiber.dispose()).unwrap();
}

// FBR-19：更新校验失败保留运行中的原配置及效应。
#[test]
fn invalid_update_preserves_active_configuration() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let received = values.clone();
    let def = ComponentDef::validated("validated", Inject::new(), validate, move |_, value| {
        received.lock().unwrap().push(value);
        async { Ok(()) }
    });
    let source = Source { ok: true, extra: 0 };
    let fiber = root.mount_with(&def, source.clone()).unwrap();
    host.finish(fiber.settled()).unwrap();
    assert!(
        host.finish(fiber.update(Source {
            ok: false,
            extra: 1
        }))
        .is_err()
    );
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(fiber.config::<Source>().as_ref().unwrap().as_ref(), &source);
    assert_eq!(values.lock().unwrap().len(), 1);
    host.finish(fiber.dispose()).unwrap();
}

// FBR-20：结构化校验错误保留字段路径，并按路径生成消息。
#[test]
fn validation_error_preserves_issues_and_paths() {
    let validation = ValidationError {
        issues: vec![
            ValidationIssue {
                message: "not a number".into(),
                path: vec!["items".into(), "0".into()],
            },
            ValidationIssue {
                message: "required".into(),
                path: Vec::new(),
            },
        ],
    };
    let error = Error::from(validation.clone());
    assert_eq!(
        error.message.as_ref(),
        "invalid config:\n  - not a number (at items.0)\n  - required"
    );
    assert_eq!(error.validation.as_deref(), Some(&validation));
    assert!(Error::new("another error").validation.is_none());
}

fn validated_listener(
    root: &Context,
    accept: bool,
    applied: Arc<Mutex<Vec<Resolved>>>,
    received: Arc<Mutex<Vec<Resolved>>>,
) -> cordis::Fiber {
    let def = ComponentDef::validated("validated", Inject::new(), validate, move |ctx, value| {
        applied.lock().unwrap().push(value);
        let received = received.clone();
        async move {
            ctx.on_waterfall::<InternalUpdate>(
                move |raw: RawConfig, next| {
                    received
                        .lock()
                        .unwrap()
                        .push(raw.downcast_ref::<Resolved>().unwrap().clone());
                    if accept {
                        Ok(Box::pin(async { Ok(()) }))
                    } else {
                        next.run()
                    }
                },
                Default::default(),
            )?;
            Ok(())
        }
    });
    root.mount_with(&def, Source { ok: true, extra: 0 })
        .unwrap()
}

// FBR-21：更新监听器收到校验后的值，实例保留原始输入。
#[test]
fn update_listeners_receive_resolved_configuration() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::new(Mutex::new(Vec::new()));
    let fiber = validated_listener(&root, false, applied.clone(), received.clone());
    host.finish(fiber.settled()).unwrap();
    let source = Source { ok: true, extra: 1 };
    host.finish(fiber.update(source.clone())).unwrap();
    assert_eq!(
        *received.lock().unwrap(),
        [Resolved {
            source: source.clone(),
            resolved: true
        }]
    );
    assert_eq!(fiber.config::<Source>().as_ref().unwrap().as_ref(), &source);
    assert_eq!(applied.lock().unwrap().len(), 2);
    host.finish(fiber.dispose()).unwrap();
}

// FBR-22：监听器接受更新后保留实例；显式重启读取最新原始配置。
#[test]
fn handled_update_preserves_source_for_later_restart() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let applied = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::new(Mutex::new(Vec::new()));
    let fiber = validated_listener(&root, true, applied.clone(), received.clone());
    host.finish(fiber.settled()).unwrap();
    let source = Source { ok: true, extra: 1 };
    host.finish(fiber.update(source.clone())).unwrap();
    assert_eq!(
        *received.lock().unwrap(),
        [Resolved {
            source: source.clone(),
            resolved: true
        }]
    );
    assert_eq!(applied.lock().unwrap().len(), 1);
    assert_eq!(fiber.config::<Source>().as_ref().unwrap().as_ref(), &source);
    host.finish(fiber.restart()).unwrap();
    assert_eq!(applied.lock().unwrap().last().unwrap().source, source);
    assert_eq!(applied.lock().unwrap().len(), 2);
    host.finish(fiber.dispose()).unwrap();
}

// A-3、A-9：内核状态监听器的 panic 后调和任务继续推进。
#[test]
fn status_listener_panic_preserves_reconciliation() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.once::<InternalStatus>(|_| panic!("status listener"))
        .unwrap();
    let fiber = root.inject(Inject::new(), |_| async { Ok(()) }).unwrap();
    host.finish(fiber.settled()).unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(
        root.errors().last().unwrap().message.as_ref(),
        "status listener"
    );
    host.finish(fiber.dispose()).unwrap();
}
