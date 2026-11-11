use cordis::{
    ComponentDef, Context, Error, ExporterOptions, Inject, LogLevel, LogMessage, LogValue,
    LoggerKey, LoggerMeta, ServiceKey,
};
use cordis_testkit::TestExecutor;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

type Captured = Arc<Mutex<Vec<LogMessage>>>;
fn setup() -> (Arc<TestExecutor>, Context, Captured) {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let messages = captured.clone();
    root.logger()
        .exporter(
            ExporterOptions {
                default_level: Some(LogLevel::Debug),
                ..Default::default()
            },
            move |message| messages.lock().unwrap().push(message.clone()),
        )
        .unwrap();
    (host, root, captured)
}
fn text(message: &LogMessage) -> &str {
    match &message.args[0] {
        LogValue::Text(text) => text,
        LogValue::Error(error) => &error.message,
    }
}

#[test]
fn log01_bounded_buffer_stays_chronological() {
    let (host, root, _) = setup();
    let buffer_handle = root.logger();
    buffer_handle.set_buffer_size(2);
    root.logger().info("one");
    host.advance(Duration::from_millis(1));
    root.logger().info("two");
    host.advance(Duration::from_millis(1));
    root.logger().info("three");
    let messages = buffer_handle.buffer();
    assert_eq!(
        messages.iter().map(text).collect::<Vec<_>>(),
        ["two", "three"]
    );
    assert_eq!(
        messages
            .iter()
            .map(|message| (message.sn, message.ts))
            .collect::<Vec<_>>(),
        [(2, Duration::from_millis(1)), (3, Duration::from_millis(2))]
    );
    buffer_handle.set_buffer_size(1);
    root.logger().info("four");
    assert_eq!(
        buffer_handle.buffer().iter().map(text).collect::<Vec<_>>(),
        ["four"]
    );
    buffer_handle.set_buffer_size(0);
    root.logger().info("five");
    assert!(buffer_handle.buffer().is_empty());
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn log02_exporter_disposal_removes_its_own_registration() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let first = Arc::new(Mutex::new(Vec::new()));
    let second = Arc::new(Mutex::new(Vec::new()));
    let values = first.clone();
    let first_effect = root
        .logger()
        .exporter(ExporterOptions::default(), move |message| {
            values.lock().unwrap().push(message.clone())
        })
        .unwrap();
    let values = second.clone();
    let second_effect = root
        .logger()
        .exporter(ExporterOptions::default(), move |message| {
            values.lock().unwrap().push(message.clone())
        })
        .unwrap();
    host.finish(first_effect.dispose()).unwrap();
    root.logger().info("one");
    assert!(first.lock().unwrap().is_empty());
    assert_eq!(second.lock().unwrap().len(), 1);
    host.finish(second_effect.dispose()).unwrap();
    root.logger().info("two");
    assert_eq!(second.lock().unwrap().len(), 1);
    assert_eq!(root.logger().exporter_count(), 0);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn log03_logger_defaults_to_current_fiber_name() {
    let (host, root, captured) = setup();
    root.logger().debug("hello");
    assert_eq!(captured.lock().unwrap().len(), 1);
    assert_eq!(captured.lock().unwrap()[0].name, "root");
    assert_eq!(
        captured.lock().unwrap()[0].fiber.upgrade().unwrap(),
        root.fiber()
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn log04_explicit_name_has_priority() {
    let (host, root, captured) = setup();
    root.intercept::<LoggerKey>(LoggerMeta {
        name: Some("intercept".into()),
        level: None,
    })
    .logger()
    .named("custom")
    .debug("hello");
    assert_eq!(captured.lock().unwrap().len(), 1);
    assert_eq!(captured.lock().unwrap()[0].name, "custom");
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn log05_intercept_supplies_logger_name() {
    let (host, root, captured) = setup();
    root.intercept::<LoggerKey>(LoggerMeta {
        name: Some("intercepted".into()),
        level: None,
    })
    .logger()
    .debug("hello");
    assert_eq!(captured.lock().unwrap().len(), 1);
    assert_eq!(captured.lock().unwrap()[0].name, "intercepted");
    host.finish(root.fiber().dispose()).unwrap();
}

struct Foo;
impl ServiceKey for Foo {
    const NAME: &'static str = "foo";
    type Value = ();
    type Meta = ();
}
struct Bar;
impl ServiceKey for Bar {
    const NAME: &'static str = "bar";
    type Value = ();
    type Meta = ();
}

fn foo(root: &Context, inject: Inject) -> cordis::Fiber {
    root.mount_with(
        &ComponentDef::new("foo:driver", inject, |ctx, ()| async move {
            ctx.provide::<Foo>(Arc::new(()))?;
            Ok(())
        }),
        (),
    )
    .unwrap()
}

#[test]
fn log06_service_method_uses_service_fiber_name() {
    let (host, root, captured) = setup();
    let provider = foo(&root, Inject::new());
    host.finish(provider.settled()).unwrap();
    root.get::<Foo>().unwrap().logger().debug("from action");
    let messages = captured.lock().unwrap();
    assert_eq!(messages[0].name, "foo:driver");
    assert_eq!(messages[0].fiber.upgrade().unwrap(), provider);
    drop(messages);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn log07_outer_intercept_overrides_service_logger_name() {
    let (host, root, captured) = setup();
    let provider = foo(&root, Inject::new());
    host.finish(provider.settled()).unwrap();
    let usage = root.intercept::<LoggerKey>(LoggerMeta {
        name: Some("caller-override".into()),
        level: None,
    });
    usage.get::<Foo>().unwrap().logger().debug("from action");
    assert_eq!(captured.lock().unwrap()[0].name, "caller-override");
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn log08_nested_service_restores_outer_logger_name() {
    let (host, root, captured) = setup();
    let bar = root
        .mount_with(
            &ComponentDef::new("bar:driver", Inject::new(), |ctx, ()| async move {
                ctx.provide::<Bar>(Arc::new(()))?;
                Ok(())
            }),
            (),
        )
        .unwrap();
    let provider = foo(&root, Inject::new().with::<Bar>());
    host.finish(bar.settled()).unwrap();
    host.finish(provider.settled()).unwrap();
    let caller = root
        .inject(Inject::new().with::<Foo>(), |ctx| async move {
            let service = ctx.get::<Foo>()?;
            service.dependency::<Bar>()?.logger().debug("from bar");
            service.logger().debug("from foo");
            Ok(())
        })
        .unwrap();
    host.finish(caller.settled()).unwrap();
    assert_eq!(
        captured
            .lock()
            .unwrap()
            .iter()
            .map(|message| (message.name.clone(), text(message).to_owned()))
            .collect::<Vec<_>>(),
        [
            ("bar:driver".into(), "from bar".into()),
            ("foo:driver".into(), "from foo".into())
        ]
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn log09_initialization_uses_component_name() {
    let (host, root, captured) = setup();
    let provider = root
        .mount_with(
            &ComponentDef::new("FooDriver", Inject::new(), |ctx, ()| async move {
                ctx.logger().debug("from init");
                Ok(())
            }),
            (),
        )
        .unwrap();
    host.finish(provider.settled()).unwrap();
    assert_eq!(captured.lock().unwrap()[0].name, "foo-driver");
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn log10_empty_aggregate_is_logged_as_itself() {
    let (host, root, captured) = setup();
    root.logger().warn(Error::new("empty aggregate"));
    let messages = captured.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(text(&messages[0]), "empty aggregate");
    drop(messages);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn log11_aggregate_members_and_error_causes_keep_order() {
    let (host, root, captured) = setup();
    let mut aggregate = Error::new("aggregate");
    aggregate.causes.push(Error::new("first"));
    aggregate.causes.push(Error::new("second"));
    root.logger().warn(aggregate);
    root.logger()
        .warn(Error::new("outer").with_cause(Error::new("cause")));
    assert_eq!(
        captured
            .lock()
            .unwrap()
            .iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["first", "second", "cause", "outer"]
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn exporter_filter_precedence_and_reentrant_callback() {
    let (host, root, captured) = setup();
    let context = root.clone();
    let filtered = Arc::new(Mutex::new(Vec::new()));
    let seen = filtered.clone();
    root.logger()
        .exporter(
            ExporterOptions {
                levels: HashMap::from([("quiet".into(), LogLevel::Error)]),
                default_level: Some(LogLevel::Warn),
            },
            move |message| {
                assert!(context.logger().exporter_count() >= 1);
                seen.lock().unwrap().push(message.clone());
            },
        )
        .unwrap();
    let logger = root
        .intercept::<LoggerKey>(LoggerMeta {
            name: None,
            level: Some(LogLevel::Debug),
        })
        .logger();
    logger.named("quiet").warn("skip named");
    logger.info("skip default");
    logger.warn("keep warning");
    logger.named("quiet").error("keep error");
    assert_eq!(
        filtered
            .lock()
            .unwrap()
            .iter()
            .map(text)
            .collect::<Vec<_>>(),
        ["keep warning", "keep error"]
    );
    assert_eq!(captured.lock().unwrap().len(), 4);
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn kernel_error_reporting_preserves_lifecycle_after_exporter_panic() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.logger()
        .exporter(ExporterOptions::default(), |_| panic!("export failed"))
        .unwrap();
    let child = root
        .inject(Inject::new(), |_| async {
            Err(Error::new("component failed"))
        })
        .unwrap();
    assert_eq!(
        host.finish(child.settled()).unwrap_err().message.as_ref(),
        "component failed"
    );
    assert_eq!(child.state(), cordis::FiberState::Failed);
    assert_eq!(
        root.errors()
            .iter()
            .map(|error| error.message.as_ref())
            .collect::<Vec<_>>(),
        ["component failed", "export failed"]
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn completed_submission_panic_keeps_disposed_fiber_as_log_source() {
    use cordis::{Clock, Spawn, Task};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct CompletingHost {
        executor: Arc<TestExecutor>,
        panic_after_completion: AtomicBool,
    }

    impl Spawn for CompletingHost {
        fn spawn(&self, task: Task) {
            self.executor.spawn(task);
            self.executor.run_until_stalled();
            assert!(
                !self.panic_after_completion.swap(false, Ordering::SeqCst),
                "host panicked after task completion"
            );
        }
    }

    impl Clock for CompletingHost {
        fn now(&self) -> Duration {
            self.executor.now()
        }

        fn sleep(&self, duration: Duration) -> Task {
            self.executor.sleep(duration)
        }
    }

    let host = Arc::new(CompletingHost {
        executor: TestExecutor::new(),
        panic_after_completion: AtomicBool::new(false),
    });
    let root = Context::new(host.clone());
    let component = ComponentDef::new("child:driver", Inject::new(), |_, ()| async { Ok(()) });
    let child = root.mount_with(&component, ()).unwrap();
    host.executor.finish(child.settled()).unwrap();
    host.panic_after_completion.store(true, Ordering::SeqCst);
    host.executor.finish(child.dispose()).unwrap();
    assert_eq!(child.state(), cordis::FiberState::Disposed);
    let messages = root.logger().buffer();
    assert_eq!(messages.len(), 1);
    assert_eq!(text(&messages[0]), "host panicked after task completion");
    assert_eq!(messages[0].name, "child:driver");
    assert_eq!(messages[0].fiber.upgrade().unwrap(), child);
    drop(child);
    assert!(messages[0].fiber.upgrade().is_none());
    host.executor.finish(root.fiber().dispose()).unwrap();
}
