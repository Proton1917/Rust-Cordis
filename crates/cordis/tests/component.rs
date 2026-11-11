use cordis::{
    Component, ComponentDef, Config, Context, EffectMeta, Event, FiberState, Inject, Result,
    SyncKind, ValidationError,
};
use cordis_testkit::TestExecutor;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Tick;
impl Event for Tick {
    const NAME: &'static str = "tick";
    type Args = ();
    type Output = ();
    type Kind = SyncKind;
}

#[derive(Clone)]
struct Text(String);
impl Config for Text {
    type Raw = String;
    fn resolve(raw: &String) -> std::result::Result<Self, ValidationError> {
        Ok(Self(raw.clone()))
    }
}
struct Typed;
impl Component for Typed {
    const NAME: &'static str = "typed";
    type Config = Text;
    async fn apply(ctx: Context, config: Text) -> Result {
        ctx.extension::<Mutex<Vec<String>>>()
            .unwrap()
            .lock()
            .unwrap()
            .push(config.0);
        Ok(())
    }
}

// CMP-01：闭包组件收到挂载配置。
#[test]
fn functional_component_receives_configuration() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let received = values.clone();
    let component = ComponentDef::new("", Inject::new(), move |_, config: String| {
        received.lock().unwrap().push(config);
        async { Ok(()) }
    });
    let fiber = root.mount_with(&component, "bar".to_owned()).unwrap();
    host.finish(fiber.settled()).unwrap();
    assert_eq!(*values.lock().unwrap(), ["bar"]);
    host.finish(fiber.dispose()).unwrap();
}

// CMP-02：Component trait 将具名组件的配置与执行函数关联。
#[test]
fn typed_component_receives_configuration() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone()).extend(Mutex::new(Vec::<String>::new()));
    let fiber = root.mount::<Typed>("foo".into()).unwrap();
    host.finish(fiber.settled()).unwrap();
    assert_eq!(
        *root
            .extension::<Mutex<Vec<String>>>()
            .unwrap()
            .lock()
            .unwrap(),
        ["foo"]
    );
    host.finish(fiber.dispose()).unwrap();
}

// CMP-05：失活上下文在清理回调内拒绝新增登记。
#[test]
fn disposed_context_rejects_new_registrations() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let errors = Arc::new(Mutex::new(Vec::new()));
    let received = errors.clone();
    let fiber = root
        .inject(Inject::new(), move |ctx| {
            let received = received.clone();
            async move {
                ctx.clone().effect("cleanup", move |fx| {
                    fx.defer(move || {
                        received.lock().unwrap().push(
                            ctx.inject(Inject::new(), |_| async { Ok(()) })
                                .unwrap_err()
                                .message
                                .to_string(),
                        );
                        received.lock().unwrap().push(
                            ctx.effect("late", |_| Ok(()))
                                .err()
                                .unwrap()
                                .message
                                .to_string(),
                        );
                        received
                            .lock()
                            .unwrap()
                            .push(ctx.on::<Tick>(|_| None).err().unwrap().message.to_string());
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(fiber.settled()).unwrap();
    host.finish(fiber.dispose()).unwrap();
    assert_eq!(
        *errors.lock().unwrap(),
        vec!["cannot create effect on inactive context"; 3]
    );
}

// CMP-06、S-FBR-4：上下文名称从最近的具名祖先解析。
#[test]
fn context_debug_uses_nearest_component_name() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    assert_eq!(format!("{root:?}"), "Context <root>");
    let names = Arc::new(Mutex::new(Vec::new()));
    for name in ["", "foo", "bar", "Qux"] {
        let received = names.clone();
        let def = ComponentDef::new(name, Inject::new(), move |ctx, ()| {
            received.lock().unwrap().push(format!("{ctx:?}"));
            async { Ok(()) }
        });
        host.finish(root.mount_with(&def, ()).unwrap().settled())
            .unwrap();
    }
    assert_eq!(
        *names.lock().unwrap(),
        [
            "Context <root>",
            "Context <foo>",
            "Context <bar>",
            "Context <Qux>"
        ]
    );
    host.finish(root.fiber().dispose()).unwrap();
}

// CMP-07：登记表枚举存活组件，并在最后一个实例释放后清除运行时。
#[test]
fn registry_tracks_runtime_instances() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let def = ComponentDef::new("registered", Inject::new(), |_, ()| async { Ok(()) });
    assert!(root.registry().is_empty());
    let a = root.mount_with(&def, ()).unwrap();
    let b = root.mount_with(&def, ()).unwrap();
    host.finish(a.settled()).unwrap();
    host.finish(b.settled()).unwrap();
    assert_eq!(root.registry().keys(), [def.id()]);
    assert_eq!(root.registry().get(&def).unwrap().id(), def.id());
    assert_eq!(root.registry().fibers(&def), [a.clone(), b.clone()]);
    host.finish(a.dispose()).unwrap();
    assert_eq!(
        root.registry().fibers(&def).as_slice(),
        std::slice::from_ref(&b)
    );
    host.finish(b.dispose()).unwrap();
    assert!(root.registry().is_empty());
}

fn nested(root: &Context, count: Arc<AtomicUsize>) -> cordis::Fiber {
    root.inject(Inject::new(), move |ctx| {
        let count = count.clone();
        async move {
            let first = count.clone();
            ctx.on::<Tick>(move |_| {
                first.fetch_add(1, Ordering::SeqCst);
                None
            })?;
            let child = ctx.inject(Inject::new(), move |ctx| {
                let count = count.clone();
                async move {
                    let second = count.clone();
                    ctx.on::<Tick>(move |_| {
                        second.fetch_add(1, Ordering::SeqCst);
                        None
                    })?;
                    let leaf = ctx.inject(Inject::new(), move |ctx| {
                        let count = count.clone();
                        async move {
                            ctx.on::<Tick>(move |_| {
                                count.fetch_add(1, Ordering::SeqCst);
                                None
                            })?;
                            Ok(())
                        }
                    })?;
                    leaf.settled().await
                }
            })?;
            child.settled().await
        }
    })
    .unwrap()
}

// CMP-08、T-LIF-8、T-LIF-9：父组件释放级联移除子组件效应。
#[test]
fn nested_components_release_their_event_handlers() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let count = Arc::new(AtomicUsize::new(0));
    let root_count = count.clone();
    root.on::<Tick>(move |_| {
        root_count.fetch_add(1, Ordering::SeqCst);
        None
    })
    .unwrap();
    let fiber = nested(&root, count.clone());
    host.finish(fiber.settled()).unwrap();
    assert_eq!(root.registry().len(), 3);
    root.emit::<Tick>(&());
    assert_eq!(count.load(Ordering::SeqCst), 4);
    host.finish(fiber.dispose()).unwrap();
    assert!(root.registry().is_empty());
    root.emit::<Tick>(&());
    assert_eq!(count.load(Ordering::SeqCst), 5);
    host.finish(fiber.dispose()).unwrap();
    root.emit::<Tick>(&());
    assert_eq!(count.load(Ordering::SeqCst), 6);
    host.finish(root.fiber().dispose()).unwrap();
}

// CMP-09：重复挂载的监听器和运行时登记均可恢复。
#[test]
fn component_disposal_restores_registration_snapshot() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let before = root.fiber().effects();
    let count = Arc::new(AtomicUsize::new(0));
    let fiber = nested(&root, count.clone());
    host.finish(fiber.settled()).unwrap();
    root.emit::<Tick>(&());
    assert_eq!(count.load(Ordering::SeqCst), 3);
    host.finish(fiber.dispose()).unwrap();
    assert_eq!(root.fiber().effects(), before);
    assert!(root.registry().is_empty());
    let again = nested(&root, count.clone());
    host.finish(again.settled()).unwrap();
    root.emit::<Tick>(&());
    assert_eq!(count.load(Ordering::SeqCst), 6);
    host.finish(again.dispose()).unwrap();
}

// CMP-10、S-FBR-17：根纤程释放后继续承载新的组件。
#[test]
fn root_disposal_releases_children_and_remains_active() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let count = Arc::new(AtomicUsize::new(0));
    let released = count.clone();
    let fiber = root
        .inject(Inject::new(), move |ctx| {
            let released = released.clone();
            async move {
                ctx.effect("resource", |fx| {
                    fx.defer(move || {
                        released.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(fiber.settled()).unwrap();
    assert_eq!(root.fiber().effects().len(), 1);
    host.finish(root.fiber().dispose()).unwrap();
    assert_eq!(root.fiber().state(), FiberState::Active);
    assert_eq!(fiber.state(), FiberState::Disposed);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(root.fiber().effects().is_empty());
    host.finish(root.fiber().dispose()).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let next = root.inject(Inject::new(), |_| async { Ok(()) }).unwrap();
    host.finish(next.settled()).unwrap();
    assert_eq!(next.state(), FiberState::Active);
    host.finish(next.dispose()).unwrap();
}

// DSP-01：组件释放按效应元信息回收资源，重复释放保持结果。
#[test]
fn component_disposes_its_effect_once() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let count = Arc::new(AtomicUsize::new(0));
    let released = count.clone();
    let fiber = root
        .inject(Inject::new(), move |ctx| {
            let released = released.clone();
            async move {
                ctx.effect("test", |fx| {
                    fx.defer(move || {
                        released.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                })?;
                Ok(())
            }
        })
        .unwrap();
    host.finish(fiber.settled()).unwrap();
    assert_eq!(
        fiber.effects(),
        [EffectMeta {
            label: "test".into(),
            children: Vec::new()
        }]
    );
    host.finish(fiber.dispose()).unwrap();
    host.finish(fiber.dispose()).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
