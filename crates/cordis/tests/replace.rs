use cordis::{
    ComponentDef, Context, Error, FiberState, Inject, InternalReplace, ServiceKey, ValidationError,
};
use cordis_testkit::TestExecutor;
use std::sync::{Arc, Mutex};

struct Dep;
impl ServiceKey for Dep {
    const NAME: &'static str = "dep";
    type Value = usize;
    type Meta = ();
}
struct Missing;
impl ServiceKey for Missing {
    const NAME: &'static str = "missing";
    type Value = ();
    type Meta = ();
}

fn plain(name: &'static str) -> ComponentDef {
    ComponentDef::new(name, Inject::new(), |_, _: usize| async { Ok(()) })
}

fn recording(
    name: &'static str,
    inject: Inject,
    values: Arc<Mutex<Vec<(&'static str, usize)>>>,
) -> ComponentDef {
    ComponentDef::new(name, inject, move |_, value: usize| {
        values.lock().unwrap().push((name, value));
        async { Ok(()) }
    })
}

// RPL-01：替换保持纤程身份，回收旧实现并以原始配置加载新实现。
#[test]
fn replacement_preserves_fiber_identity_and_configuration() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let released = Arc::new(Mutex::new(Vec::new()));
    let cleaned = released.clone();
    let old = ComponentDef::new("old", Inject::new(), move |ctx, value: usize| {
        let cleaned = cleaned.clone();
        async move {
            ctx.effect("resource", move |fx| {
                fx.defer(move || {
                    cleaned.lock().unwrap().push(value);
                    Ok(())
                })
            })?;
            Ok(())
        }
    });
    let applied = Arc::new(Mutex::new(Vec::new()));
    let next = recording("next", Inject::new(), applied.clone());
    let fiber = root.mount_with(&old, 1usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    let moved = root.registry().replace(&old, &next);
    assert_eq!(moved.as_slice(), std::slice::from_ref(&fiber));
    host.finish(fiber.settled()).unwrap();
    assert_eq!(*released.lock().unwrap(), [1]);
    assert_eq!(*applied.lock().unwrap(), [("next", 1)]);
    assert_eq!(fiber.component_id(), Some(next.id()));
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(root.registry().keys(), [next.id()]);
    host.finish(fiber.dispose()).unwrap();
}

// RPL-02：替换通知中的实例已指向新组件。
#[test]
fn replace_event_observes_moved_implementation() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = plain("old");
    let next = plain("next");
    let observed = Arc::new(Mutex::new(Vec::new()));
    let events = observed.clone();
    root.on::<InternalReplace>(move |fiber| {
        events.lock().unwrap().push(fiber.component_id());
        None
    })
    .unwrap();
    let a = root.mount_with(&old, 1usize).unwrap();
    let b = root.mount_with(&old, 2usize).unwrap();
    host.finish(a.settled()).unwrap();
    host.finish(b.settled()).unwrap();
    assert_eq!(root.registry().replace(&old, &next), [a.clone(), b.clone()]);
    assert_eq!(
        *observed.lock().unwrap(),
        [Some(next.id()), Some(next.id())]
    );
    host.finish(a.settled()).unwrap();
    host.finish(b.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}

// RPL-03：替换后按新组件的配置解析函数处理原始输入。
#[test]
fn replacement_uses_new_configuration_schema() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = ComponentDef::validated(
        "old",
        Inject::new(),
        |raw: &usize| Ok::<_, ValidationError>((*raw, 1usize)),
        |_, _| async { Ok(()) },
    );
    let values = Arc::new(Mutex::new(Vec::new()));
    let received = values.clone();
    let next = ComponentDef::validated(
        "next",
        Inject::new(),
        |raw: &usize| Ok::<_, ValidationError>((*raw, 2usize)),
        move |_, resolved| {
            received.lock().unwrap().push(resolved);
            async { Ok(()) }
        },
    );
    let fiber = root.mount_with(&old, 7usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    root.registry().replace(&old, &next);
    host.finish(fiber.settled()).unwrap();
    assert_eq!(*values.lock().unwrap(), [(7, 2)]);
    assert_eq!(*fiber.config::<usize>().unwrap(), 7);
    host.finish(fiber.dispose()).unwrap();
}

// RPL-04：替换后的新增声明决定目标满足性。
#[test]
fn replacement_adopts_new_dependency_specification() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = plain("old");
    let values = Arc::new(Mutex::new(Vec::new()));
    let next = recording("next", Inject::new().with::<Dep>(), values.clone());
    let fiber = root.mount_with(&old, 1usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    root.registry().replace(&old, &next);
    host.finish(fiber.settled()).unwrap();
    assert_eq!(fiber.inject().names(), ["dep"]);
    assert_eq!(fiber.state(), FiberState::Pending);
    assert!(values.lock().unwrap().is_empty());
    root.provide::<Dep>(Arc::new(3)).unwrap();
    host.finish(fiber.settled()).unwrap();
    assert_eq!(*values.lock().unwrap(), [("next", 1)]);
    host.finish(root.fiber().dispose()).unwrap();
}

// RPL-05：替换移除缺失依赖后激活等待中的实例。
#[test]
fn replacement_removing_missing_dependency_activates_fiber() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = ComponentDef::new(
        "old",
        Inject::new().with::<Missing>(),
        |_, _: usize| async { Ok(()) },
    );
    let values = Arc::new(Mutex::new(Vec::new()));
    let next = recording("next", Inject::new(), values.clone());
    let fiber = root.mount_with(&old, 1usize).unwrap();
    assert_eq!(fiber.state(), FiberState::Pending);
    root.registry().replace(&old, &next);
    host.finish(fiber.settled()).unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(*values.lock().unwrap(), [("next", 1)]);
    host.finish(fiber.dispose()).unwrap();
}

// RPL-06：替换声明为已可用服务后激活等待中的实例。
#[test]
fn replacement_to_available_dependency_activates_fiber() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Dep>(Arc::new(1)).unwrap();
    let old = ComponentDef::new(
        "old",
        Inject::new().with::<Missing>(),
        |_, _: usize| async { Ok(()) },
    );
    let values = Arc::new(Mutex::new(Vec::new()));
    let next = recording("next", Inject::new().with::<Dep>(), values.clone());
    let fiber = root.mount_with(&old, 1usize).unwrap();
    assert_eq!(fiber.state(), FiberState::Pending);
    root.registry().replace(&old, &next);
    host.finish(fiber.settled()).unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(*values.lock().unwrap(), [("next", 1)]);
    host.finish(root.fiber().dispose()).unwrap();
}

// RPL-07：旧声明的依赖占用在替换卸载时释放。
#[test]
fn replacement_releases_dropped_dependency() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let provider = root
        .inject(Inject::new(), |ctx| async move {
            ctx.provide::<Dep>(Arc::new(1))?;
            Ok(())
        })
        .unwrap();
    host.finish(provider.settled()).unwrap();
    let old = ComponentDef::new("old", Inject::new().with::<Dep>(), |_, _: usize| async {
        Ok(())
    });
    let values = Arc::new(Mutex::new(Vec::new()));
    let next = recording("next", Inject::new(), values.clone());
    let fiber = root.mount_with(&old, 1usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    root.registry().replace(&old, &next);
    host.finish(fiber.settled()).unwrap();
    host.finish(provider.restart()).unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(*values.lock().unwrap(), [("next", 1)]);
    host.finish(root.fiber().dispose()).unwrap();
}

// RPL-08：提供方实现替换后依赖方重新读取新绑定。
#[test]
fn replaced_provider_reloads_its_consumer() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = ComponentDef::new("old", Inject::new(), |ctx, _: usize| async move {
        ctx.provide::<Dep>(Arc::new(1))?;
        Ok(())
    });
    let next = ComponentDef::new("next", Inject::new(), |ctx, _: usize| async move {
        ctx.provide::<Dep>(Arc::new(2))?;
        Ok(())
    });
    let provider = root.mount_with(&old, 0usize).unwrap();
    host.finish(provider.settled()).unwrap();
    let values = Arc::new(Mutex::new(Vec::new()));
    let received = values.clone();
    let consumer = root
        .inject(Inject::new().with::<Dep>(), move |ctx| {
            let received = received.clone();
            async move {
                received.lock().unwrap().push(*ctx.get::<Dep>()?);
                Ok(())
            }
        })
        .unwrap();
    host.finish(consumer.settled()).unwrap();
    root.registry().replace(&old, &next);
    host.finish(provider.settled()).unwrap();
    host.finish(consumer.settled()).unwrap();
    assert_eq!(*values.lock().unwrap(), [1, 2]);
    assert_eq!(consumer.state(), FiberState::Active);
    host.finish(root.fiber().dispose()).unwrap();
}

// RPL-09：运行时合并保留目标组件已有实例。
#[test]
fn replacement_merges_existing_runtime_instances() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = plain("old");
    let values = Arc::new(Mutex::new(Vec::new()));
    let next = recording("next", Inject::new(), values.clone());
    let a = root.mount_with(&old, 1usize).unwrap();
    let b = root.mount_with(&next, 2usize).unwrap();
    host.finish(a.settled()).unwrap();
    host.finish(b.settled()).unwrap();
    root.registry().replace(&old, &next);
    host.finish(a.settled()).unwrap();
    assert_eq!(root.registry().len(), 1);
    assert_eq!(a.component_id(), b.component_id());
    assert_eq!(root.registry().fibers(&next), [b, a]);
    assert_eq!(*values.lock().unwrap(), [("next", 2), ("next", 1)]);
    host.finish(root.fiber().dispose()).unwrap();
}

// RPL-10：替换清除原实现的失败状态。
#[test]
fn replacement_recovers_failed_fiber() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = ComponentDef::new("old", Inject::new(), |_, _: usize| async {
        Err(Error::new("failed"))
    });
    let values = Arc::new(Mutex::new(Vec::new()));
    let next = recording("next", Inject::new(), values.clone());
    let fiber = root.mount_with(&old, 1usize).unwrap();
    assert!(host.finish(fiber.settled()).is_err());
    root.registry().replace(&old, &next);
    host.finish(fiber.settled()).unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    assert_eq!(*values.lock().unwrap(), [("next", 1)]);
    host.finish(fiber.dispose()).unwrap();
}

// RPL-11：新实现加载失败时向等待者交回错误。
#[test]
fn replacement_failure_is_observable() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = plain("old");
    let next = ComponentDef::new("next", Inject::new(), |_, _: usize| async {
        Err(Error::new("failed"))
    });
    let fiber = root.mount_with(&old, 0usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    root.registry().replace(&old, &next);
    assert_eq!(
        host.finish(fiber.settled()).unwrap_err().message.as_ref(),
        "failed"
    );
    assert_eq!(fiber.state(), FiberState::Failed);
    host.finish(fiber.dispose()).unwrap();
}

// RPL-12：旧组件键转发到新定义及其依赖规格。
#[test]
fn old_component_forwards_to_new_definition() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let old = recording("old", Inject::new(), values.clone());
    let next = recording("next", Inject::new().with::<Dep>(), values.clone());
    let first = root.mount_with(&old, 1usize).unwrap();
    host.finish(first.settled()).unwrap();
    root.registry().replace(&old, &next);
    let second = root.mount_with(&old, 2usize).unwrap();
    assert_eq!(second.inject().names(), ["dep"]);
    assert_eq!(second.component_id(), Some(next.id()));
    assert_eq!(root.registry().get(&old).unwrap().id(), next.id());
    assert_eq!(root.registry().keys(), [next.id()]);
    root.provide::<Dep>(Arc::new(1)).unwrap();
    host.finish(first.settled()).unwrap();
    host.finish(second.settled()).unwrap();
    let mut applied = values.lock().unwrap().clone();
    applied.sort_unstable();
    assert_eq!(applied, [("next", 1), ("next", 2), ("old", 1)]);
    host.finish(root.fiber().dispose()).unwrap();
}

// RPL-13：转发关系适用于首次挂载之前。
#[test]
fn unregistered_component_can_be_forwarded() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let old = recording("old", Inject::new(), values.clone());
    let next = recording("next", Inject::new(), values.clone());
    assert!(root.registry().replace(&old, &next).is_empty());
    assert!(root.registry().is_empty());
    let fiber = root.mount_with(&old, 7usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    assert_eq!(*values.lock().unwrap(), [("next", 7)]);
    host.finish(fiber.dispose()).unwrap();
}

// RPL-14：旧键沿连续转发关系定位最后一个定义。
#[test]
fn replacement_follows_alias_chain() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let a = plain("a");
    let b = plain("b");
    let c = plain("c");
    let fiber = root.mount_with(&a, 0usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    root.registry().replace(&a, &b);
    root.registry().replace(&a, &c);
    host.finish(fiber.settled()).unwrap();
    assert_eq!(fiber.component_id(), Some(c.id()));
    assert_eq!(root.registry().get(&a).unwrap().id(), c.id());
    assert_eq!(root.registry().keys(), [c.id()]);
    host.finish(fiber.dispose()).unwrap();
}

// RPL-15：目标组件重新成为实现时清除自身转发关系。
#[test]
fn replacement_can_switch_back_to_previous_component() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let values = Arc::new(Mutex::new(Vec::new()));
    let a = recording("a", Inject::new(), values.clone());
    let b = plain("b");
    let fiber = root.mount_with(&a, 1usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    root.registry().replace(&a, &b);
    root.registry().replace(&b, &a);
    host.finish(fiber.settled()).unwrap();
    assert_eq!(fiber.component_id(), Some(a.id()));
    assert_eq!(root.registry().keys(), [a.id()]);
    assert_eq!(*values.lock().unwrap(), [("a", 1), ("a", 1)]);
    host.finish(fiber.dispose()).unwrap();
}

// RPL-16：替换目标的最后一个实例释放后注销运行时。
#[test]
fn disposal_unregisters_replacement_runtime() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    let old = plain("old");
    let next = plain("next");
    let fiber = root.mount_with(&old, 0usize).unwrap();
    host.finish(fiber.settled()).unwrap();
    root.registry().replace(&old, &next);
    host.finish(fiber.settled()).unwrap();
    host.finish(fiber.dispose()).unwrap();
    assert!(root.registry().is_empty());
    assert!(!root.registry().contains(&next));
}
