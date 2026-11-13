use cordis::{Clock, Context, FiberState, Spawn, Task};
use cordis_supervisor::{
    PeerOptions,
    manager::{
        Arguments, CallContext, GroupSpec, InstanceSpec, LocalReply, LocalService, Manager,
        ManagerOptions, RestartPolicy,
    },
};
use cordis_testkit::cleanup::{FixtureLease, SharedFixture, TestGuard};
use cordis_up::{
    ObjectRef, Value, WireError, payload,
    rpc::{HandlerFuture, RemoteObject},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

struct Executor(Instant);
impl Spawn for Executor {
    fn spawn(&self, task: Task) {
        tokio::spawn(task);
    }
}
impl Clock for Executor {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, duration: Duration) -> Task {
        Box::pin(tokio::time::sleep(duration))
    }
}

fn integer(value: &Value) -> u64 {
    u64::try_from(value.as_integer().expect("u64 result")).unwrap()
}
fn encoded<T: serde::Serialize>(value: &T) -> Value {
    Value::serialized(value).unwrap()
}

#[derive(Clone)]
struct Ready {
    context: u64,
    object: RemoteObject,
    version: u64,
}
#[derive(Clone)]
struct Observed {
    instance: String,
    connection: u64,
    context: u64,
    phase: String,
    value: Value,
    at: Instant,
}

struct Control {
    ready: Mutex<HashMap<String, Ready>>,
    events: Mutex<Vec<Observed>>,
    changed: watch::Sender<u64>,
}
impl Control {
    fn new() -> Arc<Self> {
        let (changed, _) = watch::channel(0);
        Arc::new(Self {
            ready: Mutex::new(HashMap::new()),
            events: Mutex::new(Vec::new()),
            changed,
        })
    }
    async fn ready_after(&self, name: &str, previous: u64) -> Ready {
        let mut changed = self.changed.subscribe();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let current = self.ready.lock().get(name).cloned();
                if let Some(ready) = current
                    && ready.context != previous
                {
                    return ready;
                }
                changed.changed().await.unwrap();
            }
        })
        .await
        .expect("instance ready notification")
    }
    async fn phase_after(&self, name: &str, phase: &str, after: usize) -> Observed {
        let mut changed = self.changed.subscribe();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = self
                    .events
                    .lock()
                    .iter()
                    .skip(after)
                    .find(|event| event.instance == name && event.phase == phase)
                    .cloned();
                if let Some(event) = event {
                    return event;
                }
                changed.changed().await.unwrap();
            }
        })
        .await
        .expect("lifecycle phase notification")
    }
}

impl LocalService for Control {
    fn invoke<'a>(
        &'a self,
        context: &'a CallContext,
        method: &'a str,
        args: &'a Arguments,
    ) -> HandlerFuture<'a, Result<LocalReply, WireError>> {
        Box::pin(async move {
            let (phase, value) = match method {
                "ready" => {
                    let (reference, count, version): (ObjectRef, u64, u64) =
                        Value::Array(args.values.clone())
                            .deserialized()
                            .map_err(|error| WireError::new(2, error.to_string()))?;
                    let ready = Ready {
                        context: context.context,
                        object: args.import(reference.0)?,
                        version,
                    };
                    let previous = self.ready.lock().insert(context.instance.clone(), ready);
                    drop(previous);
                    ("ready".to_owned(), count.into())
                }
                "trace" => Value::Array(args.values.clone())
                    .deserialized::<(String, Value)>()
                    .map_err(|error| WireError::new(2, error.to_string()))?,
                _ => return Err(WireError::new(3, "unknown control method")),
            };
            self.events.lock().push(Observed {
                instance: context.instance.clone(),
                connection: context.connection,
                context: context.context,
                phase,
                value,
                at: Instant::now(),
            });
            self.changed.send_modify(|revision| *revision += 1);
            Ok(Value::Null.into())
        })
    }
}

fn plugin() -> FixtureLease<PathBuf> {
    static PLUGIN: SharedFixture<PathBuf> = SharedFixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = root
        .parent()
        .and_then(Path::parent)
        .expect("workspace package layout");
    PLUGIN.get_or_init(workspace, "manager-fixture", |guard| {
        let _build = cordis_testkit::build::shared_build_lock(workspace).unwrap();
        let destination = cordis_testkit::build::shared_target_dir(workspace);
        let result = std::process::Command::new(env!("CARGO"))
            .args(["build", "--manifest-path"])
            .arg(root.join("fixtures/manager-plugin/Cargo.toml"))
            .arg("--target-dir")
            .arg(&destination)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        cordis_testkit::build::snapshot_artifacts(
            guard,
            "manager-fixture",
            "debug",
            &["cordis-manager-fixture"],
        )
        .unwrap()
        .join("cordis-manager-fixture")
    })
}

fn group(name: &str, mode: &str, plugin_name: &str, restart: RestartPolicy) -> GroupSpec {
    GroupSpec {
        name: name.into(),
        command: vec![
            plugin().display().to_string(),
            mode.into(),
            plugin_name.into(),
        ],
        directory: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        peer: PeerOptions::default(),
        restart,
    }
}
fn no_restart() -> RestartPolicy {
    RestartPolicy {
        max: 0,
        backoff: Duration::from_millis(500),
    }
}
fn configuration(initial: u64) -> Value {
    Value::Map(vec![
        ("initial".into(), initial.into()),
        ("cleanup_ms".into(), 25u64.into()),
        ("refs".into(), Value::Array(vec![23u64.into()])),
    ])
}
fn instance(name: &str, group: &str, plugin: &str, component: &str, config: Value) -> InstanceSpec {
    InstanceSpec {
        name: name.into(),
        group: group.into(),
        plugin: plugin.into(),
        component: component.into(),
        config,
    }
}
fn options() -> ManagerOptions {
    ManagerOptions {
        handshake_timeout: Duration::from_secs(5),
        apply_timeout: Duration::from_secs(5),
        cleanup_timeout: Duration::from_secs(5),
        export_timeout: Duration::from_secs(5),
        invoke_timeout: Duration::from_secs(5),
        swap_timeout: Duration::from_secs(5),
        confirm: Duration::from_millis(10),
        ..ManagerOptions::default()
    }
}
async fn setup(options: ManagerOptions) -> (Manager, Arc<Control>) {
    let manager = Manager::new(Context::new(Arc::new(Executor(Instant::now()))), options)
        .await
        .unwrap();
    let control = Control::new();
    manager
        .provide_builtin(
            "fixture.control@1.0",
            vec!["ready".into(), "trace".into()],
            control.clone(),
        )
        .unwrap();
    (manager, control)
}
async fn invoke(ready: &Ready, method: &str, args: Vec<Value>) -> Value {
    try_invoke(ready, method, args)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "invoke {method}, context {}, connection {}: {error:?}",
                ready.context,
                ready.object.peer().id()
            )
        })
}

async fn try_invoke(ready: &Ready, method: &str, args: Vec<Value>) -> Result<Value, WireError> {
    let result = ready
        .object
        .peer()
        .request(
            "invoke",
            encoded(&payload::Invoke {
                object: ready.object.id(),
                method: method.into(),
                args,
                refs: Vec::new(),
            }),
            Duration::from_secs(10),
        )
        .await?;
    Ok(result
        .value()
        .deserialized::<payload::InvokeResult>()
        .unwrap()
        .value)
}

async fn core_call<T: serde::Serialize>(ready: &Ready, method: &str, params: &T) -> Value {
    invoke(ready, "core", vec![method.into(), encoded(params)]).await
}

async fn derive(
    ready: &Ready,
    context: u64,
    options: payload::ContextOptions,
) -> payload::ContextDerived {
    core_call(
        ready,
        "context_derive",
        &payload::ContextDerive {
            context,
            isolate: options.isolate,
            intercept: options.intercept,
        },
    )
    .await
    .deserialized()
    .unwrap()
}

async fn context_retired(ready: &Ready, context: u64) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match try_invoke(
                ready,
                "core",
                vec!["config".into(), encoded(&payload::Context { context })],
            )
            .await
            {
                Err(problem) if problem.code == 4 => break,
                Err(problem) => panic!("unexpected context error: {problem}"),
                Ok(_) => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .expect("context retirement notification");
}
async fn until(duration: Duration, condition: impl Fn() -> bool) {
    tokio::time::timeout(duration, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("bounded state transition");
}

// M10：同组依赖、事件登记和committed清理都由唯一核心决定。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn core_orders_same_group_cleanup_and_rebuilds_dependencies() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-order");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    manager
        .attach_group(group("subject", "v1", "manager-fixture", no_restart()))
        .await
        .unwrap();
    let a = manager
        .mount(
            instance("a", "subject", "manager-fixture", "a", configuration(10)),
            None,
        )
        .await
        .unwrap();
    a.settled().await.unwrap();
    assert_eq!(a.state(), FiberState::Pending);
    assert!(!control.ready.lock().contains_key("a"));
    let b = manager
        .mount(
            instance("b", "subject", "manager-fixture", "b", configuration(10)),
            None,
        )
        .await
        .unwrap();
    b.settled().await.unwrap();
    a.settled().await.unwrap();
    let old_a = control.ready_after("a", 0).await;
    let old_b = control.ready_after("b", 0).await;
    assert_eq!(
        invoke(&old_a, "observed", Vec::new()).await,
        Value::Array(vec![10u64.into(), 1u64.into()])
    );
    manager
        .emit_builtin("fixture.changed@1.0", 3u64.into())
        .unwrap();
    control.phase_after("a", "event-sync", 0).await;
    invoke(&old_b, "emit", Vec::new()).await;
    control.phase_after("a", "event-async", 0).await;
    assert_eq!(
        invoke(&old_a, "events", Vec::new()).await,
        Value::Array(vec![3u64.into(), 5u64.into()])
    );
    let epoch = b.epoch();
    let invalid = Value::Map(vec![("invalid".into(), Value::Bool(true))]);
    assert_eq!(manager.update("b", invalid).await.unwrap_err().code, 1003);
    assert_eq!(b.epoch(), epoch);
    assert_eq!(integer(&invoke(&old_b, "add", vec![7u64.into()]).await), 17);
    let after = control.events.lock().len();
    let report = manager
        .replace_group(group("subject", "v2", "manager-fixture", no_restart()))
        .await
        .unwrap();
    assert!(report.succeeded);
    let new_a = control.ready_after("a", old_a.context).await;
    let new_b = control.ready_after("b", old_b.context).await;
    assert_eq!(new_b.version, 2);
    assert_eq!(integer(&invoke(&new_b, "get", Vec::new()).await), 17);
    assert_eq!(
        invoke(&new_a, "observed", Vec::new()).await,
        Value::Array(vec![17u64.into(), 2u64.into()])
    );
    assert_eq!(a.state(), FiberState::Active);
    assert_eq!(b.state(), FiberState::Active);
    {
        let events = control.events.lock();
        let retired = &events[after..];
        let a_end = retired
            .iter()
            .position(|event| event.instance == "a" && event.phase == "a-clean-end")
            .unwrap();
        let b_end = retired
            .iter()
            .position(|event| event.instance == "b" && event.phase == "b-clean")
            .unwrap();
        assert!(a_end < b_end);
        assert_eq!(integer(&retired[a_end].value), 17);
    }
    manager.shutdown().await.unwrap();
    case.success();
}

/// ADR-0041：两条真实进程连接分别承担后端与页面组件。
async fn page_groups() -> (Manager, Arc<Control>) {
    let (manager, control) = setup(options()).await;
    manager
        .attach_group(group("backend", "v1", "backend-plugin", no_restart()))
        .await
        .unwrap();
    manager
        .attach_group(group("page", "v1", "page-plugin", no_restart()))
        .await
        .unwrap();
    (manager, control)
}

async fn mount_backend(manager: &Manager, name: &str) {
    manager
        .mount(
            instance(
                name,
                "backend",
                "backend-plugin",
                "independent",
                configuration(7),
            ),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
}

fn page_spec(name: &str, initial: u64) -> InstanceSpec {
    instance(
        name,
        "page",
        "page-plugin",
        "independent",
        configuration(initial),
    )
}

fn page_scope(realm: &str) -> payload::ContextOptions {
    payload::ContextOptions {
        isolate: vec![payload::Isolation {
            interface: "fixture.independent@1.0".into(),
            realm: Some(realm.into()),
        }],
        intercept: Vec::new(),
    }
}

/// DOC-15 §11.2、ADR-0041：跨连接子实例保留父级效应，父完成前执行实际远端清理。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deployment_children_finish_before_parent_disposal() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "page-parent");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = page_groups().await;
    mount_backend(&manager, "parent").await;
    assert_eq!(
        integer(
            &manager
                .invoke_service("fixture.independent@1.0", "add", vec![3u64.into()])
                .await
                .unwrap()
        ),
        10,
    );
    let child = manager
        .mount_child_instance(
            "parent".into(),
            page_spec("page-child", 42),
            None,
            page_scope("page-one"),
        )
        .await
        .unwrap();
    child.settled().await.unwrap();
    let ready = control.ready_after("page-child", 0).await;
    assert_eq!(integer(&invoke(&ready, "get", Vec::new()).await), 42);
    manager.dispose("parent").await.unwrap();
    assert_eq!(
        try_invoke(&ready, "get", Vec::new())
            .await
            .unwrap_err()
            .code,
        4
    );
    assert!(
        manager
            .snapshot()
            .instances
            .iter()
            .all(|instance| instance.name != "page-child")
    );
    manager.shutdown().await.unwrap();
    case.success();
}

/// DOC-15 §11.2：核心 Owner 按后端存在信号重建，后端停用等待页面的实际清理。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn core_owner_tracks_backend_presence_and_releases_page() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "page-owner");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = page_groups().await;
    let current = Arc::new(Mutex::new(None));
    let observed = current.clone();
    let definition = cordis::ComponentDef::new(
        "web-owner",
        cordis::Inject::new().with_named("cordis.instance.backend"),
        move |context, ()| {
            *observed.lock() = Some(context);
            async { Ok(()) }
        },
    );
    let owner = manager.context().mount_with(&definition, ()).unwrap();
    assert_eq!(owner.state(), FiberState::Pending);
    mount_backend(&manager, "backend").await;
    owner.settled().await.unwrap();
    assert_eq!(owner.state(), FiberState::Active);
    let context = current.lock().clone().unwrap();
    let child = manager
        .mount_owned_child(
            context,
            page_spec("owned-page", 43),
            None,
            page_scope("page-owner"),
        )
        .await
        .unwrap();
    child.settled().await.unwrap();
    let ready = control.ready_after("owned-page", 0).await;
    assert_eq!(integer(&invoke(&ready, "get", Vec::new()).await), 43);
    freeze_page(&manager, &ready).await;
    manager.dispose("backend").await.unwrap();
    assert_eq!(owner.state(), FiberState::Pending);
    assert_eq!(
        try_invoke(&ready, "get", Vec::new())
            .await
            .unwrap_err()
            .code,
        4
    );
    assert!(
        manager
            .snapshot()
            .instances
            .iter()
            .all(|instance| instance.name != "owned-page")
    );
    owner.dispose().await.unwrap();
    manager.shutdown().await.unwrap();
    case.success();
}

/// DOC-11 SW-5：快照持有期间写入被冻结，明确解冻后恢复真实服务调用。
async fn freeze_page(manager: &Manager, ready: &Ready) {
    let snapshot = manager
        .freeze_instances(&["owned-page".into()])
        .await
        .unwrap();
    assert_eq!(integer(&snapshot.states()["owned-page"].data), 43);
    assert_eq!(
        try_invoke(ready, "add", vec![1u64.into()])
            .await
            .unwrap_err()
            .code,
        4
    );
    manager.thaw_snapshot(&snapshot).await.unwrap();
    assert_eq!(integer(&invoke(ready, "add", vec![1u64.into()]).await), 44);
}

/// ADR-0041：页面名称复用后，旧父效应清理保留新实例的目录记录。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_parent_cleanup_preserves_reused_child_name() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "page-reused-name");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = page_groups().await;
    mount_backend(&manager, "parent-a").await;
    let view = manager
        .derive_context(&manager.context(), &page_scope("parent-b"))
        .unwrap();
    manager
        .mount_in(
            view,
            instance(
                "parent-b",
                "backend",
                "backend-plugin",
                "independent",
                configuration(8),
            ),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    manager
        .mount_child_instance(
            "parent-a".into(),
            page_spec("shared-page", 41),
            None,
            page_scope("old-page"),
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let old = control.ready_after("shared-page", 0).await;
    manager.dispose("shared-page").await.unwrap();
    let child = manager
        .mount_child_instance(
            "parent-b".into(),
            page_spec("shared-page", 42),
            None,
            page_scope("new-page"),
        )
        .await
        .unwrap();
    child.settled().await.unwrap();
    let current = control.ready_after("shared-page", old.context).await;
    manager.dispose("parent-a").await.unwrap();
    assert!(
        manager
            .snapshot()
            .instances
            .iter()
            .any(|instance| instance.name == "shared-page")
    );
    assert_eq!(integer(&invoke(&current, "get", Vec::new()).await), 42);
    manager
        .dispose_owned_child("shared-page", &child)
        .await
        .unwrap();
    assert!(
        manager
            .snapshot()
            .instances
            .iter()
            .all(|instance| instance.name != "shared-page")
    );
    manager.shutdown().await.unwrap();
    case.success();
}

/// ADR-0041：挂载通知触发父级释放时，登记失败会等待子级回收并移除目录。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_closing_during_child_attachment_reclaims_catalog_entry() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "page-attach-close");
    let _fixture = plugin().for_case(&case);
    let (manager, _) = page_groups().await;
    let current = Arc::new(Mutex::new(None));
    let captured = current.clone();
    let definition = cordis::ComponentDef::new(
        "closing-owner",
        cordis::Inject::new(),
        move |context, ()| {
            *captured.lock() = Some(context);
            async { Ok(()) }
        },
    );
    let owner = manager.context().mount_with(&definition, ()).unwrap();
    owner.settled().await.unwrap();
    let retained = owner.clone();
    manager
        .context()
        .on::<cordis::InternalPlugin>(move |fiber| {
            if fiber.is_alive() && fiber.name() == "page/page-plugin/independent" {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        tokio::time::timeout(Duration::from_secs(5), retained.dispose())
                            .await
                            .unwrap()
                            .unwrap();
                    })
                });
            }
            None
        })
        .unwrap();
    let parent = current.lock().clone().unwrap();
    let result = manager
        .mount_owned_child(
            parent,
            page_spec("opening-page", 1),
            None,
            page_scope("closing-page"),
        )
        .await;
    assert!(result.is_err(), "父级释放后挂载交回错误");
    assert!(
        manager
            .snapshot()
            .instances
            .iter()
            .all(|instance| instance.name != "opening-page")
    );
    manager.shutdown().await.unwrap();
    case.success();
}

// CP-6：真实SIGKILL回收提供方，依赖转Pending，独立进程保持纪元，重启继承最近检查点。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn process_failure_releases_dependencies_and_restores_checkpoint() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-failure");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    manager
        .attach_group(group(
            "provider",
            "v1",
            "provider-fixture",
            RestartPolicy {
                max: 1,
                backoff: Duration::from_millis(150),
            },
        ))
        .await
        .unwrap();
    manager
        .attach_group(group("consumer", "v1", "consumer-fixture", no_restart()))
        .await
        .unwrap();
    manager
        .attach_group(group("healthy", "v1", "healthy-fixture", no_restart()))
        .await
        .unwrap();
    let b = manager
        .mount(
            instance("b", "provider", "provider-fixture", "b", configuration(10)),
            None,
        )
        .await
        .unwrap();
    b.settled().await.unwrap();
    let a = manager
        .mount(
            instance("a", "consumer", "consumer-fixture", "a", configuration(10)),
            None,
        )
        .await
        .unwrap();
    a.settled().await.unwrap();
    let healthy = manager
        .mount(
            instance(
                "healthy",
                "healthy",
                "healthy-fixture",
                "independent",
                configuration(900),
            ),
            None,
        )
        .await
        .unwrap();
    healthy.settled().await.unwrap();
    let old_b = control.ready_after("b", 0).await;
    let old_a = control.ready_after("a", 0).await;
    let independent = control.ready_after("healthy", 0).await;
    let healthy_epoch = healthy.epoch();
    assert_eq!(
        integer(&invoke(&old_b, "add", vec![31u64.into()]).await),
        41
    );
    let snapshot = manager.checkpoint().await.unwrap();
    assert_eq!(integer(&snapshot["b"].data), 41);
    assert_eq!(integer(&invoke(&old_b, "add", vec![1u64.into()]).await), 42);
    let pid = manager
        .snapshot()
        .groups
        .iter()
        .find(|group| group.name == "provider")
        .unwrap()
        .pid
        .unwrap();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    until(Duration::from_secs(5), || a.state() == FiberState::Pending).await;
    assert_eq!(healthy.epoch(), healthy_epoch);
    assert_eq!(integer(&invoke(&independent, "get", Vec::new()).await), 900);
    let restored = control.ready_after("b", old_b.context).await;
    control.ready_after("a", old_a.context).await;
    assert_eq!(integer(&invoke(&restored, "get", Vec::new()).await), 41);
    assert_eq!(healthy.epoch(), healthy_epoch);
    assert_eq!(
        manager
            .snapshot()
            .groups
            .iter()
            .find(|group| group.name == "provider")
            .unwrap()
            .restarts,
        1
    );
    manager.shutdown().await.unwrap();
    case.success();
}

// CP-6、CP-11：五次真实退避重启后停止，恢复值来自core持有的最近快照。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_limit_uses_backoff_and_latest_snapshot() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-restart");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    manager
        .attach_group(group(
            "subject",
            "v1",
            "manager-fixture",
            RestartPolicy::default(),
        ))
        .await
        .unwrap();
    manager
        .mount(
            instance("b", "subject", "manager-fixture", "b", configuration(10)),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let ready = control.ready_after("b", 0).await;
    invoke(&ready, "add", vec![31u64.into()]).await;
    manager.checkpoint().await.unwrap();
    let after = control.events.lock().len();
    manager
        .replace_group(group(
            "subject",
            "crash-loop",
            "manager-fixture",
            RestartPolicy::default(),
        ))
        .await
        .unwrap();
    until(Duration::from_secs(35), || {
        manager
            .snapshot()
            .groups
            .iter()
            .any(|group| group.name == "subject" && group.state == "failed" && group.restarts == 5)
    })
    .await;
    let events: Vec<_> = control
        .events
        .lock()
        .iter()
        .skip(after)
        .filter(|event| event.instance == "b" && event.phase == "ready")
        .cloned()
        .collect();
    assert_eq!(
        events.len(),
        6,
        "groups: {:?}; ready connections and contexts: {:?}",
        manager.snapshot().groups,
        events
            .iter()
            .map(|event| (event.connection, event.context))
            .collect::<Vec<_>>()
    );
    assert!(events.iter().all(|event| integer(&event.value) == 41));
    for (index, pair) in events.windows(2).enumerate() {
        assert!(pair[1].at.duration_since(pair[0].at) >= Duration::from_millis(500 * (1 << index)));
        assert_ne!(pair[0].connection, pair[1].connection);
        assert_ne!(pair[0].context, pair[1].context);
    }
    manager.shutdown().await.unwrap();
    case.success();
}

// 组替换的候选apply失败后，由core恢复旧进程实现与导出的真实状态。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_group_replacement_rolls_back_state() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-rollback");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    let original = manager
        .attach_group(group("subject", "v1", "manager-fixture", no_restart()))
        .await
        .unwrap();
    manager
        .mount(
            instance("b", "subject", "manager-fixture", "b", configuration(10)),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let old = control.ready_after("b", 0).await;
    invoke(&old, "add", vec![19u64.into()]).await;
    assert_eq!(
        manager
            .replace_group(group("subject", "fail", "manager-fixture", no_restart()))
            .await
            .unwrap_err()
            .code,
        1001
    );
    let restored = control.ready_after("b", old.context).await;
    assert_eq!(restored.version, 1);
    assert_eq!(integer(&invoke(&restored, "get", Vec::new()).await), 29);
    let snapshot = manager.snapshot();
    assert_eq!(snapshot.groups[0].connection, original.connection);
    assert!(
        snapshot
            .last_replace
            .iter()
            .any(|report| report.group == "subject" && report.rolled_back && !report.succeeded)
    );
    manager.shutdown().await.unwrap();
    case.success();
}

// 已接收的组替换由core有界动作队列持有，丢弃调用方等待继续完成既定事务。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_replacement_survives_waiter_cancellation() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-cancel");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    let original = manager
        .attach_group(group("subject", "v1", "manager-fixture", no_restart()))
        .await
        .unwrap();
    let mut config = configuration(10);
    config
        .as_map_mut()
        .unwrap()
        .push(("apply_ms".into(), 200u64.into()));
    manager
        .mount(
            instance("b", "subject", "manager-fixture", "b", config),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let old = control.ready_after("b", 0).await;
    let after = control.events.lock().len();
    let waiting = tokio::spawn({
        let manager = manager.clone();
        async move {
            manager
                .replace_group(group("subject", "v2", "manager-fixture", no_restart()))
                .await
        }
    });
    let starting = control.phase_after("b", "apply-start", after).await;
    assert_ne!(Some(starting.connection), original.connection);
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    let new = control.ready_after("b", old.context).await;
    until(Duration::from_secs(5), || {
        manager
            .snapshot()
            .last_replace
            .iter()
            .any(|report| report.group == "subject" && report.succeeded)
    })
    .await;
    assert_eq!(new.version, 2);
    let old_pid = nix::unistd::Pid::from_raw(original.pid.unwrap() as i32);
    until(Duration::from_secs(5), || {
        matches!(
            nix::sys::signal::kill(old_pid, None),
            Err(nix::errno::Errno::ESRCH)
        )
    })
    .await;
    manager.shutdown().await.unwrap();
    case.success();
}

// 长寿命登记额度由真实效应和外部句柄共同释放；元数据按保留字节预算拒绝。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registration_and_metadata_quotas_recover_after_cleanup() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-quotas");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(ManagerOptions {
        max_registrations_per_context: 7,
        ..options()
    })
    .await;
    manager
        .attach_group(group("subject", "v1", "manager-fixture", no_restart()))
        .await
        .unwrap();
    manager
        .mount(
            instance("b", "subject", "manager-fixture", "b", configuration(10)),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let ready = control.ready_after("b", 0).await;
    assert_eq!(
        invoke(&ready, "quota", Vec::new()).await,
        Value::Array(vec![8u64.into(), 1u64.into()])
    );
    assert_eq!(integer(&invoke(&ready, "cycles", Vec::new()).await), 3);
    assert_eq!(integer(&invoke(&ready, "get", Vec::new()).await), 10);
    manager.shutdown().await.unwrap();

    let (manager, _) = setup(ManagerOptions {
        max_retained_metadata_bytes: 4096,
        ..options()
    })
    .await;
    manager
        .attach_group(group("subject", "v1", "manager-fixture", no_restart()))
        .await
        .unwrap();
    let mut config = configuration(10);
    config
        .as_map_mut()
        .unwrap()
        .push(("metadata_size".into(), 8192u64.into()));
    let fiber = manager
        .mount(
            instance("large", "subject", "manager-fixture", "b", config),
            None,
        )
        .await
        .unwrap();
    assert!(fiber.settled().await.is_err());
    let snapshot = manager.snapshot();
    assert_eq!(
        snapshot
            .instances
            .iter()
            .find(|instance| instance.name == "large")
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .code,
        8
    );
    manager.shutdown().await.unwrap();
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn derived_scopes_cache_actual_binding_and_own_child_lifetime() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-scopes");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    manager
        .attach_group(group("subject", "v1", "manager-fixture", no_restart()))
        .await
        .unwrap();
    let mut config = configuration(10);
    config
        .as_map_mut()
        .unwrap()
        .push(("scoped".into(), true.into()));
    manager
        .mount(
            instance("b", "subject", "manager-fixture", "b", config),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    manager
        .mount(
            instance("a", "subject", "manager-fixture", "a", configuration(10)),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let provider = control.ready_after("b", 0).await;
    let consumer = control.ready_after("a", 0).await;
    let first = derive(
        &consumer,
        consumer.context,
        payload::ContextOptions {
            isolate: vec![payload::Isolation {
                interface: "fixture.independent@1".into(),
                realm: None,
            }],
            intercept: vec![payload::Interception {
                interface: "fixture.counter@1".into(),
                meta: Value::Map(vec![
                    ("prefix".into(), "old".into()),
                    ("limit".into(), 3u64.into()),
                ]),
            }],
        },
    )
    .await;
    let nested = derive(
        &consumer,
        first.context,
        payload::ContextOptions {
            isolate: Vec::new(),
            intercept: vec![payload::Interception {
                interface: "fixture.counter@1.0".into(),
                meta: Value::Map(vec![("prefix".into(), "new".into())]),
            }],
        },
    )
    .await;
    let first_get: payload::GetResult = core_call(
        &consumer,
        "get",
        &payload::Get {
            context: nested.context,
            interface: "fixture.counter@1.0".into(),
        },
    )
    .await
    .deserialized()
    .unwrap();
    let second_get: payload::GetResult = core_call(
        &consumer,
        "get",
        &payload::Get {
            context: nested.context,
            interface: "fixture.counter@1.0".into(),
        },
    )
    .await
    .deserialized()
    .unwrap();
    assert_eq!(
        first_get.local.as_ref().unwrap().object,
        second_get.local.as_ref().unwrap().object
    );
    let result: payload::InvokeResult = core_call(
        &consumer,
        "invoke",
        &payload::Invoke {
            object: first_get.object,
            method: "meta".into(),
            args: Vec::new(),
            refs: Vec::new(),
        },
    )
    .await
    .deserialized()
    .unwrap();
    assert_eq!(
        result.value,
        Value::Map(vec![
            ("prefix".into(), "new".into()),
            ("limit".into(), 3u64.into()),
            (
                "cordis.caller".into(),
                Value::Map(vec![("instance".into(), "a".into())])
            )
        ])
    );
    let result: payload::PluginResult = core_call(
        &consumer,
        "plugin",
        &payload::PluginRequest {
            context: nested.context,
            component: "independent".into(),
            config: configuration(42),
        },
    )
    .await
    .deserialized()
    .unwrap();
    let child_name = format!("a/independent:{}", result.instance);
    let child = control.ready_after(&child_name, 0).await;
    assert_eq!(integer(&invoke(&child, "get", Vec::new()).await), 42);
    invoke(&consumer, "release-object", vec![nested.object.into()]).await;
    invoke(&consumer, "release-object", vec![first.object.into()]).await;
    assert!(
        core_call(
            &consumer,
            "is_current",
            &payload::Context {
                context: nested.context
            }
        )
        .await
        .as_bool()
        .unwrap()
    );
    manager.dispose("a").await.unwrap();
    context_retired(&provider, nested.context).await;
    assert_eq!(
        try_invoke(&child, "get", Vec::new())
            .await
            .unwrap_err()
            .code,
        4
    );
    manager.shutdown().await.unwrap();
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn named_realms_share_bindings_and_separate_other_instances() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-realms");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    manager
        .attach_group(group("subject", "v1", "manager-fixture", no_restart()))
        .await
        .unwrap();
    let view = |name: &str| {
        manager
            .derive_context(
                &manager.context(),
                &payload::ContextOptions {
                    isolate: vec![payload::Isolation {
                        interface: "fixture.counter@1".into(),
                        realm: Some(name.into()),
                    }],
                    intercept: Vec::new(),
                },
            )
            .unwrap()
    };
    manager
        .mount_in(
            view("blue"),
            instance(
                "b-blue",
                "subject",
                "manager-fixture",
                "b",
                configuration(11),
            ),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    manager
        .mount_in(
            view("green"),
            instance(
                "b-green",
                "subject",
                "manager-fixture",
                "b",
                configuration(22),
            ),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    manager
        .mount_in(
            view("blue"),
            instance("a", "subject", "manager-fixture", "a", configuration(0)),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let consumer = control.ready_after("a", 0).await;
    assert_eq!(integer(&invoke(&consumer, "get", Vec::new()).await), 11);
    let before = consumer.context;
    manager.dispose("b-green").await.unwrap();
    assert_eq!(
        manager
            .snapshot()
            .instances
            .iter()
            .find(|instance| instance.name == "a")
            .unwrap()
            .context,
        Some(before)
    );
    manager.shutdown().await.unwrap();
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn released_view_registration_recycles_and_effect_keeps_its_context() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-views");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(ManagerOptions {
        max_registrations_per_context: 8,
        ..options()
    })
    .await;
    manager
        .attach_group(group("subject", "v1", "manager-fixture", no_restart()))
        .await
        .unwrap();
    manager
        .mount(
            instance("b", "subject", "manager-fixture", "b", configuration(10)),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let ready = control.ready_after("b", 0).await;
    for _ in 0..3 {
        let view = derive(&ready, ready.context, payload::ContextOptions::default()).await;
        invoke(&ready, "release-object", vec![view.object.into()]).await;
        context_retired(&ready, view.context).await;
    }
    let view = derive(&ready, ready.context, payload::ContextOptions::default()).await;
    let effect: payload::EffectHandle = invoke(&ready, "view-effect", vec![view.context.into()])
        .await
        .deserialized()
        .unwrap();
    invoke(&ready, "release-object", vec![view.object.into()]).await;
    assert!(
        core_call(
            &ready,
            "is_current",
            &payload::Context {
                context: view.context
            }
        )
        .await
        .as_bool()
        .unwrap()
    );
    core_call(&ready, "effect_dispose", &effect).await;
    core_call(&ready, "effect_release", &effect).await;
    context_retired(&ready, view.context).await;
    manager.shutdown().await.unwrap();
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selected_checkpoint_detach_and_shutdown_freeze_use_real_process_state() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "manager-checkpoint");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    let subject = manager
        .attach_group(group(
            "subject",
            "v1",
            "manager-fixture",
            RestartPolicy::default(),
        ))
        .await
        .unwrap();
    manager
        .attach_group(group("other", "v1", "other-plugin", no_restart()))
        .await
        .unwrap();
    manager
        .mount(
            instance("b", "subject", "manager-fixture", "b", configuration(10)),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    manager
        .mount(
            instance(
                "independent",
                "other",
                "other-plugin",
                "independent",
                configuration(40),
            ),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let provider = control.ready_after("b", 0).await;
    let independent = control.ready_after("independent", 0).await;
    let states = manager.checkpoint_instances(&["b".into()]).await.unwrap();
    assert_eq!(states.keys().collect::<Vec<_>>(), vec!["b"]);
    assert_eq!(integer(&states["b"].data), 10);
    assert_eq!(
        integer(&invoke(&provider, "add", vec![2u64.into()]).await),
        12
    );
    manager.detach_group("subject").await.unwrap();
    assert!(
        manager
            .snapshot()
            .groups
            .iter()
            .all(|group| group.name != "subject")
    );
    assert!(
        manager
            .snapshot()
            .instances
            .iter()
            .all(|instance| instance.name != "b")
    );
    assert_eq!(
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(subject.pid.unwrap() as i32),
            None
        ),
        Err(nix::errno::Errno::ESRCH)
    );
    assert_eq!(
        integer(&invoke(&independent, "add", vec![2u64.into()]).await),
        42
    );
    let states = manager.export_shutdown().await.unwrap();
    assert_eq!(integer(&states["independent"].data), 42);
    assert_eq!(
        try_invoke(&independent, "add", vec![1u64.into()])
            .await
            .unwrap_err()
            .code,
        4
    );
    manager.shutdown().await.unwrap();
    case.success();
}
