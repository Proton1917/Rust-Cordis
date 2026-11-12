#![forbid(unsafe_code)]

mod allocation;

use clap::Parser;
use cordis::{Clock, Context, FiberState, Spawn, Task};
use cordis_supervisor::{
    PeerOptions,
    manager::{
        Arguments, CallContext, GroupSpec, InstanceSpec, LocalReply, LocalService, Manager,
        ManagerOptions, RestartPolicy,
    },
};
use cordis_up::{
    ObjectRef, Value, WireError, payload,
    rpc::{HandlerFuture, RemoteObject},
};
use std::{
    error::Error,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

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

struct Artifacts {
    host: PathBuf,
    provider: PathBuf,
    scoped: PathBuf,
    version2: PathBuf,
    failed: PathBuf,
    incompatible: PathBuf,
    consumer: PathBuf,
    work: PathBuf,
}

#[derive(Parser)]
struct Selection {
    #[arg(long)]
    reuse_artifacts: bool,
    #[arg(long, conflicts_with = "reuse_artifacts")]
    build_only: bool,
    #[arg(long)]
    artifacts_dir: Option<PathBuf>,
    #[arg(long)]
    host: Option<PathBuf>,
    cases: Vec<String>,
}

fn existing_artifacts(build: &Path, host: Option<PathBuf>) -> Result<Artifacts> {
    let variant = |name: &str| {
        build
            .join("variants")
            .join(format!("{name}.{}", std::env::consts::DLL_EXTENSION))
    };
    let library = |directory: &str, name: &str| {
        build.join(directory).join("debug").join(format!(
            "{}{name}.{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_EXTENSION
        ))
    };
    let work = build.join(format!("native-run-{}", std::process::id()));
    std::fs::create_dir(&work)?;
    Ok(Artifacts {
        host: host.unwrap_or_else(|| build.join("debug/cordis-host")),
        provider: variant("provider-v1"),
        version2: variant("provider-v2"),
        failed: variant("provider-fails"),
        incompatible: variant("provider-incompatible"),
        scoped: library("fixtures-scoped", "loader_fixture"),
        consumer: library("fixtures-185", "loader_fixture_consumer"),
        work,
    })
}

fn build(host_root: &Path, build: &Path, host: Option<&Path>) -> Result<()> {
    if host.is_none() {
        run(
            Command::new("cargo")
                .args(["build", "--manifest-path"])
                .arg(host_root.join("Cargo.toml"))
                .arg("--target-dir")
                .arg(&build),
            "构建真实原生宿主",
        )?;
    }
    let fixtures = host_root.join("tests/fixtures/Cargo.toml");
    let current = build.join("fixtures-current");
    run(
        Command::new("cargo")
            .args(["build", "--manifest-path"])
            .arg(&fixtures)
            .args(["-p", "loader-fixture", "--target-dir"])
            .arg(&current),
        "用当前编译器构建提供方动态库",
    )?;
    let library = |directory: &Path, name: &str| {
        directory.join("debug").join(format!(
            "{}{name}.{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_EXTENSION
        ))
    };
    let variants = build.join("variants");
    std::fs::create_dir_all(&variants)?;
    let provider = variants.join(format!("provider-v1.{}", std::env::consts::DLL_EXTENSION));
    std::fs::copy(library(&current, "loader_fixture"), &provider)?;
    for (features, name) in [
        ("version2", "provider-v2"),
        ("version2,apply-fails", "provider-fails"),
        ("incompatible", "provider-incompatible"),
    ] {
        run(
            Command::new("cargo")
                .args(["build", "--manifest-path"])
                .arg(&fixtures)
                .args([
                    "-p",
                    "loader-fixture",
                    "--features",
                    features,
                    "--target-dir",
                ])
                .arg(&current),
            name,
        )?;
        let path = variants.join(format!("{name}.{}", std::env::consts::DLL_EXTENSION));
        std::fs::copy(library(&current, "loader_fixture"), &path)?;
    }
    let scoped = build.join("fixtures-scoped");
    run(
        Command::new("cargo")
            .args(["build", "--manifest-path"])
            .arg(&fixtures)
            .args([
                "-p",
                "loader-fixture",
                "--features",
                "scoped",
                "--target-dir",
            ])
            .arg(&scoped),
        "构建Scoped提供方动态库",
    )?;
    let previous = build.join("fixtures-185");
    run(
        Command::new("cargo")
            .args(["+1.85.0", "build", "--manifest-path"])
            .arg(&fixtures)
            .args(["-p", "loader-fixture-consumer", "--target-dir"])
            .arg(&previous),
        "用Rust 1.85.0构建使用方动态库",
    )?;
    Ok(())
}

struct Control(Mutex<Option<RemoteObject>>);

impl LocalService for Control {
    fn invoke<'a>(
        &'a self,
        _: &'a CallContext,
        method: &'a str,
        args: &'a Arguments,
    ) -> HandlerFuture<'a, std::result::Result<LocalReply, WireError>> {
        Box::pin(async move {
            if method != "offer" {
                return Err(WireError::new(3, "控制接口方法未知"));
            }
            let (object,): (ObjectRef,) = Value::Array(args.values.clone())
                .deserialized()
                .map_err(|error| WireError::new(2, error.to_string()))?;
            let previous = self.0.lock().unwrap().replace(args.import(object.0)?);
            drop(previous);
            Ok(Value::Null.into())
        })
    }
}

async fn counter_call(object: &RemoteObject, method: &str, args: Vec<Value>) -> Result<u64> {
    let request = Value::serialized(&payload::Invoke {
        object: object.id(),
        method: method.into(),
        args,
        refs: Vec::new(),
    })?;
    let response = object
        .peer()
        .request("invoke", request, Duration::from_secs(5))
        .await?;
    let result: payload::InvokeResult = response.value().deserialized()?;
    result
        .value
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| std::io::Error::other("计数接口应返回u64").into())
}

async fn offered(
    manager: &Manager,
    control: &Control,
    group: &str,
    name: &str,
) -> Result<RemoteObject> {
    let fiber = manager
        .mount(
            InstanceSpec {
                name: name.into(),
                group: group.into(),
                plugin: "loader-consumer".into(),
                component: "carrier".into(),
                config: Value::Map(Vec::new()),
            },
            None,
        )
        .await?;
    fiber.settled().await?;
    let object = control
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| std::io::Error::other("服务资源参数应到达核心"))?;
    manager.dispose(name).await?;
    Ok(object)
}

fn run(command: &mut Command, purpose: &str) -> Result<()> {
    eprintln!("{purpose}");
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("{purpose}失败：{status}")).into())
    }
}

fn group(
    name: &str,
    directory: &Path,
    artifacts: &Artifacts,
    libraries: &[&Path],
) -> Result<GroupSpec> {
    std::fs::create_dir(directory)?;
    let mut command = vec![
        artifacts.host.display().to_string(),
        "--cache-dir".into(),
        directory.join("lib").display().to_string(),
        "--request-timeout-ms".into(),
        "5000".into(),
        "--unload".into(),
        "when-unleased".into(),
    ];
    for library in libraries {
        command.push("--library".into());
        command.push(library.display().to_string());
    }
    Ok(GroupSpec {
        name: name.into(),
        command,
        directory: directory.into(),
        peer: PeerOptions::default(),
        restart: RestartPolicy {
            max: 0,
            backoff: Duration::from_millis(500),
        },
    })
}

fn check(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message.into()).into())
    }
}

async fn exercise(artifacts: &Artifacts, same_group: bool) -> Result<()> {
    let manager = Manager::new(
        Context::new(Arc::new(Executor(Instant::now()))),
        ManagerOptions {
            handshake_timeout: Duration::from_secs(5),
            apply_timeout: Duration::from_secs(5),
            cleanup_timeout: Duration::from_secs(5),
            export_timeout: Duration::from_secs(5),
            invoke_timeout: Duration::from_secs(5),
            swap_timeout: Duration::from_secs(5),
            confirm: Duration::from_millis(10),
            ..ManagerOptions::default()
        },
    )
    .await?;
    let operation = async {
        let (provider_group, consumer_group) = if same_group {
            let description = manager
                .attach_group(group(
                    "same",
                    &artifacts.work.join("same"),
                    artifacts,
                    &[&artifacts.provider, &artifacts.consumer],
                )?)
                .await?;
            let consumer = description
                .plugins
                .iter()
                .find(|plugin| plugin.name == "loader-consumer")
                .ok_or_else(|| std::io::Error::other("宿主描述缺少使用方库"))?;
            check(
                consumer.build_info.contains("1.85.0"),
                format!("编译器信息：{}", consumer.build_info),
            )?;
            eprintln!("同组原生宿主PID：{:?}", description.pid);
            ("same", "same")
        } else {
            let provider = manager
                .attach_group(group(
                    "storage",
                    &artifacts.work.join("storage"),
                    artifacts,
                    &[&artifacts.provider],
                )?)
                .await?;
            let consumer = manager
                .attach_group(group(
                    "client",
                    &artifacts.work.join("client"),
                    artifacts,
                    &[&artifacts.consumer],
                )?)
                .await?;
            eprintln!("跨组原生宿主PID：{:?}、{:?}", provider.pid, consumer.pid);
            ("storage", "client")
        };
        let provider = manager
            .mount(
                InstanceSpec {
                    name: "store".into(),
                    group: provider_group.into(),
                    plugin: "loader-fixture".into(),
                    component: "provider".into(),
                    config: Value::Map(Vec::new()),
                },
                None,
            )
            .await?;
        provider.settled().await?;
        check(
            provider.state() == FiberState::Active,
            format!("提供方状态：{:?}", provider.state()),
        )?;
        let consumer = manager
            .mount(
                InstanceSpec {
                    name: "client".into(),
                    group: consumer_group.into(),
                    plugin: "loader-consumer".into(),
                    component: "contract".into(),
                    config: Value::Map(vec![(
                        Value::Text("expected_typed".into()),
                        Value::Bool(same_group),
                    )]),
                },
                None,
            )
            .await?;
        consumer.settled().await?;
        check(
            consumer.state() == FiberState::Active,
            format!("使用方状态：{:?}", consumer.state()),
        )?;
        let states = manager.checkpoint().await?;
        let state = states
            .get("store")
            .ok_or_else(|| std::io::Error::other("提供方状态导出缺席"))?;
        let count = state
            .data
            .as_integer()
            .and_then(|value| u64::try_from(value).ok());
        check(
            count == Some(102),
            format!("跨库调用后期望102，实际{count:?}"),
        )?;
        manager.dispose("client").await?;
        manager.dispose("store").await?;
        eprintln!("验证通过：typed={same_group}，真实跨库调用100→102，状态导出102");
        Ok::<_, Box<dyn Error>>(())
    };
    let result = match tokio::time::timeout(Duration::from_secs(20), operation).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::other("原生跨库场景超过20秒").into()),
    };
    let cleanup = tokio::time::timeout(Duration::from_secs(10), manager.shutdown()).await;
    result?;
    cleanup.map_err(|_| std::io::Error::other("原生宿主关闭超过10秒"))??;
    Ok(())
}

async fn composition(artifacts: &Artifacts, same_group: bool) -> Result<()> {
    let context = Context::new(Arc::new(Executor(Instant::now())));
    let manager = Manager::new(
        context.clone(),
        ManagerOptions {
            handshake_timeout: Duration::from_secs(5),
            apply_timeout: Duration::from_secs(5),
            cleanup_timeout: Duration::from_secs(5),
            export_timeout: Duration::from_secs(5),
            invoke_timeout: Duration::from_secs(5),
            swap_timeout: Duration::from_secs(5),
            confirm: Duration::from_millis(10),
            ..ManagerOptions::default()
        },
    )
    .await?;
    let logs = Arc::new(Mutex::new(Vec::new()));
    manager.set_log_sink({
        let logs = logs.clone();
        Arc::new(move |record| logs.lock().unwrap().push(record))
    });
    let operation = async {
        let (storage, client) = if same_group {
            manager
                .attach_group(group(
                    "composition-same",
                    &artifacts.work.join("composition-same"),
                    artifacts,
                    &[&artifacts.scoped, &artifacts.consumer],
                )?)
                .await?;
            ("composition-same", "composition-same")
        } else {
            manager
                .attach_group(group(
                    "composition-storage",
                    &artifacts.work.join("composition-storage"),
                    artifacts,
                    &[&artifacts.scoped],
                )?)
                .await?;
            manager
                .attach_group(group(
                    "composition-client",
                    &artifacts.work.join("composition-client"),
                    artifacts,
                    &[&artifacts.consumer],
                )?)
                .await?;
            ("composition-storage", "composition-client")
        };
        for (name, initial, realm) in [
            ("base", 100u64, None),
            ("tenant-a", 200, Some("native-a")),
            ("tenant-b", 300, Some("native-b")),
        ] {
            let config = Value::Map(vec![
                ("initial".into(), initial.into()),
                (
                    "realm".into(),
                    realm.map_or(Value::Null, |realm| Value::Text(realm.into())),
                ),
            ]);
            let fiber = manager
                .mount(
                    InstanceSpec {
                        name: name.into(),
                        group: storage.into(),
                        plugin: "loader-fixture".into(),
                        component: "provider".into(),
                        config,
                    },
                    None,
                )
                .await?;
            fiber.settled().await?;
            check(
                fiber.state() == FiberState::Active,
                format!("隔离提供方{name}：{:?}", fiber.state()),
            )?;
        }
        let services_before = context.service_count();
        let parent = manager
            .mount(
                InstanceSpec {
                    name: "parent".into(),
                    group: client.into(),
                    plugin: "loader-consumer".into(),
                    component: "composition".into(),
                    config: Value::Map(vec![("expected_typed".into(), Value::Bool(same_group))]),
                },
                None,
            )
            .await?;
        parent.settled().await?;
        check(
            parent.state() == FiberState::Active,
            format!("组合组件状态：{:?}", parent.state()),
        )?;
        check(
            context.service_count() == services_before + 1,
            "子组件应在私有域中提供一个实际服务",
        )?;
        let states = manager.checkpoint().await?;
        for (name, expected) in [("base", 101u64), ("tenant-a", 203), ("tenant-b", 300)] {
            let value = states
                .get(name)
                .and_then(|state| state.data.as_integer())
                .and_then(|value| u64::try_from(value).ok());
            check(
                value == Some(expected),
                format!("{name}期望{expected}，实际{value:?}"),
            )?;
        }
        manager.dispose("parent").await?;
        // S-FBR-12、S-FBR-13：顶层清理并发，实例完成点等待全部子组件和效应。
        check(
            parent.state() == FiberState::Disposed,
            "父实例应在全部子组件清理后完成Disposed",
        )?;
        check(
            context.service_count() == services_before,
            "卸下父实例后私域服务应撤回",
        )?;
        {
            let logs = logs.lock().unwrap();
            check(
                logs.iter().any(|record| record.message == "child-cleanup"),
                "子组件清理日志应到达核心",
            )?;
            check(
                logs.iter()
                    .any(|record| record.message == "parent-view-cleanup"),
                "外部视图丢弃后效应捕获的视图应保持到清理",
            )?;
            let reader = logs
                .iter()
                .position(|record| record.message == "private-reader-cleanup")
                .ok_or_else(|| std::io::Error::other("私域读取子组件清理日志缺席"))?;
            let provider = logs
                .iter()
                .position(|record| record.message == "private-provider-cleanup")
                .ok_or_else(|| std::io::Error::other("私域提供子组件清理日志缺席"))?;
            check(reader < provider, "私域依赖方应先于提供方完成清理")?;
        }
        for name in ["tenant-b", "tenant-a", "base"] {
            manager.dispose(name).await?;
        }
        eprintln!(
            "原生组合通过：typed={same_group}，Scoped缓存/元数据、命名域、新域、子组件和视图清理"
        );
        Ok::<_, Box<dyn Error>>(())
    };
    let result = match tokio::time::timeout(Duration::from_secs(25), operation).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::other("原生组合场景超过25秒").into()),
    };
    let cleanup = tokio::time::timeout(Duration::from_secs(10), manager.shutdown()).await;
    result?;
    cleanup.map_err(|_| std::io::Error::other("组合宿主关闭超过10秒"))??;
    Ok(())
}

async fn swaps(artifacts: &Artifacts, threshold: bool) -> Result<()> {
    let manager = Manager::new(
        Context::new(Arc::new(Executor(Instant::now()))),
        ManagerOptions {
            handshake_timeout: Duration::from_secs(5),
            apply_timeout: Duration::from_secs(5),
            cleanup_timeout: Duration::from_secs(5),
            export_timeout: Duration::from_secs(5),
            invoke_timeout: Duration::from_secs(5),
            swap_timeout: Duration::from_secs(5),
            confirm: Duration::from_millis(1),
            retired_image_limit: if threshold { 20 } else { 100 },
            ..ManagerOptions::default()
        },
    )
    .await?;
    let control = Arc::new(Control(Mutex::new(None)));
    manager.provide_builtin("host.control@1.0", vec!["offer".into()], control.clone())?;
    let logs = Arc::new(Mutex::new(Vec::new()));
    manager.set_log_sink({
        let logs = logs.clone();
        Arc::new(move |record| logs.lock().unwrap().push(record))
    });
    let operation = async {
        let name = if threshold { "threshold" } else { "swap" };
        let group_before = manager
            .attach_group(group(
                name,
                &artifacts.work.join(name),
                artifacts,
                &[&artifacts.provider, &artifacts.consumer],
            )?)
            .await?;
        let provider = manager
            .mount(
                InstanceSpec {
                    name: "store".into(),
                    group: name.into(),
                    plugin: "loader-fixture".into(),
                    component: "provider".into(),
                    config: Value::Map(Vec::new()),
                },
                None,
            )
            .await?;
        provider.settled().await?;
        let low = manager
            .mount(
                InstanceSpec {
                    name: "low".into(),
                    group: name.into(),
                    plugin: "loader-consumer".into(),
                    component: "low".into(),
                    config: Value::Map(Vec::new()),
                },
                None,
            )
            .await?;
        low.settled().await?;
        let idle = manager
            .mount(
                InstanceSpec {
                    name: "idle".into(),
                    group: name.into(),
                    plugin: "loader-consumer".into(),
                    component: "idle".into(),
                    config: Value::Map(Vec::new()),
                },
                None,
            )
            .await?;
        idle.settled().await?;
        let idle_epoch = idle.epoch();
        if !threshold {
            let callers = manager
                .mount(
                    InstanceSpec {
                        name: "callers".into(),
                        group: name.into(),
                        plugin: "loader-consumer".into(),
                        component: "callers".into(),
                        config: Value::Map(Vec::new()),
                    },
                    None,
                )
                .await?;
            callers.settled().await?;
            wait_callers(&logs, 1).await?;
        }
        let service = offered(&manager, &control, name, "carrier").await?;
        check(
            counter_call(&service, "add", vec![37u64.into()]).await? == 137,
            "状态迁移前应实际写入137",
        )?;
        check(
            counter_call(&service, "implementation", Vec::new()).await? == 1,
            "初始库实现应为v1",
        )?;
        drop(service);
        if threshold {
            for round in 0..21 {
                let report = manager
                    .replace_library(
                        name.into(),
                        "loader-fixture".into(),
                        artifacts.version2.clone(),
                    )
                    .await?;
                check(
                    report.succeeded && report.quiescence_ms.is_some(),
                    format!("第{}次换库应确认旧库静止", round + 1),
                )?;
                if round < 20 {
                    check(idle.epoch() == idle_epoch, "组内换库应保持无关实例纪元")?;
                }
            }
            let group_after = manager
                .snapshot()
                .groups
                .into_iter()
                .find(|group| group.name == name)
                .ok_or_else(|| std::io::Error::other("分组状态缺席"))?;
            check(
                group_after.connection != group_before.connection,
                "累计超过20个退役版本应执行分组重启",
            )?;
            check(idle.epoch() != idle_epoch, "分组重启应重建组内实例")?;
            let states = manager.checkpoint().await?;
            let value = states
                .get("store")
                .and_then(|state| state.data.as_integer())
                .and_then(|value| u64::try_from(value).ok());
            check(
                value == Some(137),
                format!("分组重启应保留137，实际{value:?}"),
            )?;
            eprintln!("阈值验收通过：21轮固定换库，前20轮无关纪元保持，第21轮分组重启并继承137");
        } else {
            let low_epoch = low.epoch();
            let report = manager
                .replace_library(
                    name.into(),
                    "loader-fixture".into(),
                    artifacts.version2.clone(),
                )
                .await?;
            check(
                report.succeeded && report.quiescence_ms.is_some(),
                "v1到v2应成功并报告旧库静止",
            )?;
            check(
                low.epoch() != low_epoch && idle.epoch() == idle_epoch,
                "依赖方应重载，无关实例应保持纪元",
            )?;
            wait_callers(&logs, 2).await?;
            let libraries = manager.library_snapshots().await?;
            let old = libraries
                .iter()
                .find(|library| library.id == report.previous_library)
                .ok_or_else(|| std::io::Error::other("旧库状态缺席"))?;
            check(
                old.retired && old.quiescent && old.closed && old.leases == 0,
                "旧库应零租约、静止并关闭句柄",
            )?;
            {
                let records = logs.lock().unwrap();
                let consumer = records
                    .iter()
                    .position(|record| record.message == "consumer-cleanup:1")
                    .ok_or_else(|| std::io::Error::other("使用方清理证据缺席"))?;
                let provider = records
                    .iter()
                    .position(|record| record.message == "provider-cleanup:1")
                    .ok_or_else(|| std::io::Error::other("提供方清理证据缺席"))?;
                check(consumer < provider, "核心应安排使用方先于提供方清理")?;
            }
            let service = offered(&manager, &control, name, "after-v2").await?;
            check(
                counter_call(&service, "value", Vec::new()).await? == 137,
                "v2应继承真实写入的137",
            )?;
            check(
                counter_call(&service, "implementation", Vec::new()).await? == 2,
                "直连引用应指向v2代码",
            )?;
            drop(service);
            check(
                manager
                    .replace_library(
                        name.into(),
                        "loader-fixture".into(),
                        artifacts.failed.clone(),
                    )
                    .await
                    .is_err(),
                "apply失败候选应返回失败",
            )?;
            check(
                manager
                    .library_swap_reports()
                    .iter()
                    .any(|(_, report)| report.plugin == "loader-fixture" && report.rolled_back),
                "apply失败应完成回滚",
            )?;
            check(
                provider.state() == FiberState::Active
                    && low.state() == FiberState::Active
                    && idle.epoch() == idle_epoch,
                "回滚后实例应恢复且无关纪元保持",
            )?;
            let service = offered(&manager, &control, name, "after-rollback").await?;
            check(
                counter_call(&service, "value", Vec::new()).await? == 137
                    && counter_call(&service, "implementation", Vec::new()).await? == 2,
                "回滚应恢复v2及原状态137",
            )?;
            drop(service);
            let before = (provider.epoch(), low.epoch(), idle.epoch());
            check(
                manager
                    .replace_library(
                        name.into(),
                        "loader-fixture".into(),
                        artifacts.incompatible.clone(),
                    )
                    .await
                    .is_err(),
                "接口主版本不兼容应拒绝",
            )?;
            check(
                before == (provider.epoch(), low.epoch(), idle.epoch()),
                "兼容检查拒绝应保持全部实例纪元",
            )?;
            eprintln!("组内换库通过：真实v1→v2、清理次序、零租约、apply失败回滚和不兼容拒绝");
        }
        if !threshold {
            manager.dispose("callers").await?;
        }
        for instance in ["low", "store", "idle"] {
            manager.dispose(instance).await?;
        }
        Ok::<_, Box<dyn Error>>(())
    };
    let result = match tokio::time::timeout(Duration::from_secs(60), operation).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::other("原生换库场景超过60秒").into()),
    };
    let cleanup = tokio::time::timeout(Duration::from_secs(10), manager.shutdown()).await;
    result?;
    cleanup.map_err(|_| std::io::Error::other("换库宿主关闭超过10秒"))??;
    Ok(())
}

async fn wait_callers(
    logs: &Mutex<Vec<cordis_supervisor::manager::LogRecord>>,
    version: u64,
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let ready = {
                let logs = logs.lock().unwrap();
                (0..8).all(|index| {
                    logs.iter()
                        .any(|record| record.message == format!("caller-{index}-version-{version}"))
                })
            };
            if ready {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .map_err(|_| std::io::Error::other(format!("8线程应实际调用v{version}")))?;
    Ok(())
}

async fn faults(artifacts: &Artifacts, fault: &str) -> Result<()> {
    let manager = Manager::new(
        Context::new(Arc::new(Executor(Instant::now()))),
        ManagerOptions {
            handshake_timeout: Duration::from_secs(5),
            apply_timeout: Duration::from_secs(3),
            cleanup_timeout: Duration::from_secs(3),
            export_timeout: Duration::from_secs(3),
            invoke_timeout: Duration::from_secs(3),
            ..ManagerOptions::default()
        },
    )
    .await?;
    let control = Arc::new(Control(Mutex::new(None)));
    manager.provide_builtin("host.control@1.0", vec!["offer".into()], control.clone())?;
    let operation = async {
        let name = format!("fault-{fault}");
        let survivor_name = format!("survivor-{fault}");
        manager
            .attach_group(group(
                &survivor_name,
                &artifacts.work.join(&survivor_name),
                artifacts,
                &[&artifacts.consumer],
            )?)
            .await?;
        let survivor = manager
            .mount(
                InstanceSpec {
                    name: "survivor".into(),
                    group: survivor_name.clone(),
                    plugin: "loader-consumer".into(),
                    component: "idle".into(),
                    config: Value::Map(Vec::new()),
                },
                None,
            )
            .await?;
        survivor.settled().await?;
        let epoch = survivor.epoch();
        manager
            .attach_group(group(
                &name,
                &artifacts.work.join(&name),
                artifacts,
                &[&artifacts.provider, &artifacts.consumer],
            )?)
            .await?;
        let config = if fault == "apply-panic" {
            Value::Map(vec![("panic_apply".into(), true.into())])
        } else {
            Value::Map(vec![("fault".into(), Value::Text(fault.into()))])
        };
        let victim = manager
            .mount(
                InstanceSpec {
                    name: "victim".into(),
                    group: name.clone(),
                    plugin: "loader-fixture".into(),
                    component: "provider".into(),
                    config,
                },
                None,
            )
            .await?;
        if fault == "apply-panic" {
            check(victim.settled().await.is_err(), "apply panic应使加载失败")?;
            let snapshot = manager.snapshot();
            let status = snapshot
                .instances
                .iter()
                .find(|instance| instance.name == "victim")
                .ok_or_else(|| std::io::Error::other("故障实例状态缺席"))?;
            check(
                status.error.as_ref().is_some_and(|error| error.code == 5),
                "apply panic应保留code5",
            )?;
        } else {
            victim.settled().await?;
            let dependent = manager
                .mount(
                    InstanceSpec {
                        name: "dependent".into(),
                        group: survivor_name.clone(),
                        plugin: "loader-consumer".into(),
                        component: "dependent".into(),
                        config: Value::Map(Vec::new()),
                    },
                    None,
                )
                .await?;
            dependent.settled().await?;
            check(
                dependent.state() == FiberState::Active,
                "跨组依赖方应在故障前激活",
            )?;
            let service = offered(&manager, &control, &name, "fault-carrier").await?;
            if fault == "panic" {
                let error = counter_call(&service, "value", Vec::new())
                    .await
                    .err()
                    .ok_or_else(|| std::io::Error::other("服务panic应返回错误"))?;
                check(
                    error
                        .downcast_ref::<WireError>()
                        .is_some_and(|error| error.code == 5),
                    format!("服务panic应为code5：{error}"),
                )?;
                check(
                    counter_call(&service, "implementation", Vec::new()).await? == 1,
                    "服务panic后宿主应继续调用其他方法",
                )?;
            } else {
                let attempts = if fault == "spin" { 3 } else { 1 };
                for _ in 0..attempts {
                    let request = Value::serialized(&payload::Invoke {
                        object: service.id(),
                        method: "value".into(),
                        args: Vec::new(),
                        refs: Vec::new(),
                    })?;
                    check(
                        service
                            .peer()
                            .request("invoke", request, Duration::from_millis(200))
                            .await
                            .is_err(),
                        "原生故障调用应失败",
                    )?;
                }
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        if manager
                            .snapshot()
                            .groups
                            .iter()
                            .any(|group| group.name == name && group.state == "failed")
                        {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await
                .map_err(|_| std::io::Error::other("故障分组应被核心回收"))?;
                tokio::time::timeout(Duration::from_secs(5), async {
                    while dependent.state() != FiberState::Pending {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await
                .map_err(|_| {
                    std::io::Error::other(format!(
                        "故障分组的跨组依赖方应转为Pending，实际{:?}",
                        dependent.state()
                    ))
                })?;
            }
            drop(service);
            manager.dispose("dependent").await?;
        }
        check(
            survivor.state() == FiberState::Active && survivor.epoch() == epoch,
            "其他分组应保持Active及原纪元",
        )?;
        manager.dispose("victim").await?;
        manager.dispose("survivor").await?;
        eprintln!("原生故障隔离通过：{fault}，核心继续运行，其他分组纪元保持");
        Ok::<_, Box<dyn Error>>(())
    };
    let result = match tokio::time::timeout(Duration::from_secs(20), operation).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::other(format!("原生故障场景{fault}超过20秒")).into()),
    };
    let cleanup = tokio::time::timeout(Duration::from_secs(10), manager.shutdown()).await;
    result?;
    cleanup.map_err(|_| std::io::Error::other("故障场景清理超过10秒"))??;
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let selection = Selection::parse();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let directory = selection
        .artifacts_dir
        .unwrap_or_else(|| root.join("../../target/host-native"));
    if !selection.reuse_artifacts {
        build(&root, &directory, selection.host.as_deref())?;
    }
    if selection.build_only {
        return Ok(());
    }
    let artifacts = existing_artifacts(&directory, selection.host)?;
    let cases = if selection.cases.is_empty() {
        [
            "basic-same",
            "basic-cross",
            "scoped-same",
            "scoped-cross",
            "swap",
            "threshold",
            "allocation",
            "panic",
            "apply-panic",
            "abort",
            "segfault",
            "stack",
            "exit",
            "spin",
        ]
        .map(str::to_owned)
        .to_vec()
    } else {
        selection.cases
    };
    for case in cases {
        match case.as_str() {
            "basic-same" => exercise(&artifacts, true).await?,
            "basic-cross" => exercise(&artifacts, false).await?,
            "scoped-same" => composition(&artifacts, true).await?,
            "scoped-cross" => composition(&artifacts, false).await?,
            "swap" => swaps(&artifacts, false).await?,
            "threshold" => swaps(&artifacts, true).await?,
            "allocation" => allocation::run(&artifacts).await?,
            "panic" | "apply-panic" | "abort" | "segfault" | "stack" | "exit" | "spin" => {
                faults(&artifacts, &case).await?
            }
            _ => return Err(std::io::Error::other(format!("验收场景{case}未知")).into()),
        }
    }
    eprintln!("所选原生宿主验收场景全部通过");
    Ok(())
}
