//! DOC-15 CP-8：真实双宿主复现同步等待耗尽原生工作线程，截止后回收故障组。

use super::*;

pub(super) async fn run(artifacts: &Artifacts, mode: &str) -> Result<()> {
    let manager = Manager::new(
        Context::new(Arc::new(Executor(Instant::now()))),
        ManagerOptions {
            confirm: Duration::from_millis(10),
            ..ManagerOptions::default()
        },
    )
    .await?;
    let operation = exercise(&manager, artifacts, mode);
    let result = tokio::time::timeout(Duration::from_secs(20), operation).await;
    let cleanup = tokio::time::timeout(Duration::from_secs(10), manager.shutdown()).await;
    result.map_err(|_| std::io::Error::other("同步等待场景超过20秒"))??;
    cleanup.map_err(|_| std::io::Error::other("同步等待宿主回收超过10秒"))??;
    Ok(())
}

async fn prepare(manager: &Manager, artifacts: &Artifacts, raw: bool) -> Result<RemoteObject> {
    let control = Arc::new(Control(Mutex::new(None)));
    manager.provide_builtin(
        "host.wait-control@1.0",
        vec!["offer".into()],
        control.clone(),
    )?;
    for (name, library) in [
        ("wait-storage", &artifacts.provider),
        ("wait-client", &artifacts.consumer),
    ] {
        let description = manager
            .attach_group(group(
                name,
                &artifacts.work.join(name),
                artifacts,
                &[library],
            )?)
            .await?;
        eprintln!("同步等待场景分组 {name} PID={:?}", description.pid);
    }
    for (name, group, plugin, component) in [
        ("wait-store", "wait-storage", "loader-fixture", "provider"),
        (
            "wait-probe",
            "wait-client",
            "loader-consumer",
            "blocking-wait",
        ),
    ] {
        let fiber = manager
            .mount(
                InstanceSpec {
                    name: name.into(),
                    group: group.into(),
                    plugin: plugin.into(),
                    component: component.into(),
                    config: if component == "blocking-wait" {
                        Value::Map(vec![("raw".into(), raw.into())])
                    } else {
                        Value::Map(Vec::new())
                    },
                },
                None,
            )
            .await?;
        fiber.settled().await?;
        check(fiber.state() == FiberState::Active, "场景插件进入Active")?;
    }
    let object = control
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| std::io::Error::other("等待服务对象缺席"))?;
    Ok(object)
}

async fn exercise(manager: &Manager, artifacts: &Artifacts, mode: &str) -> Result<()> {
    let object = prepare(manager, artifacts, mode == "blocking-wait").await?;
    match mode {
        "blocking-wait" => raw_wait(manager, &object).await,
        "safe-wait" => safe_wait(&object).await,
        "legacy-wait" => legacy_wait(&object).await,
        _ => unreachable!(),
    }
}

async fn raw_wait(manager: &Manager, object: &RemoteObject) -> Result<()> {
    let started = Instant::now();
    let (first, second) = tokio::join!(call(object, "call"), call(object, "call"));
    eprintln!(
        "两个同步等待：elapsed_ms={}，结果={first:?} / {second:?}",
        started.elapsed().as_millis()
    );
    for result in [first, second, call(object, "call").await] {
        check(
            matches!(result, Err(WireError { code: 6, .. })),
            "同步等待到期返回code6",
        )?;
    }
    object.peer().closed().await;
    while !manager
        .snapshot()
        .groups
        .iter()
        .any(|g| g.name == "wait-client" && g.state == "failed")
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    eprintln!("同步等待复现：两线程占满，三次code6后分组被回收");
    Ok(())
}

async fn safe_wait(object: &RemoteObject) -> Result<()> {
    let started = Instant::now();
    let (a, b, c, d) = tokio::join!(
        call(object, "call"),
        call(object, "call"),
        call(object, "call"),
        call(object, "call")
    );
    for result in [a, b, c, d] {
        check(
            result? == Value::from(100u64),
            "四路并发等待返回真实计数100",
        )?;
    }
    eprintln!(
        "安全等待四路并发通过：elapsed_ms={}",
        started.elapsed().as_millis()
    );
    for method in ["nested", "system_thread", "task"] {
        check(
            call(object, method).await? == Value::from(100u64),
            format!("{method} 等待返回100"),
        )?;
        eprintln!("安全等待通过：{method}");
    }
    for (method, code) in [("failure", 1021), ("panics", 5)] {
        check(
            matches!(call(object, method).await, Err(error) if error.code == code),
            format!("{method} 保留错误码{code}"),
        )?;
    }
    check(
        call(object, "nested").await? == Value::from(100u64),
        "panic 后宿主继续处理调用",
    )?;
    Ok(())
}

async fn legacy_wait(object: &RemoteObject) -> Result<()> {
    check(
        matches!(call(object, "legacy").await, Err(error) if error.code == 3),
        "旧宿主在轮询前返回code3",
    )?;
    check(
        call(object, "polls").await? == Value::from(0u64),
        "旧宿主上的Future轮询数为0",
    )?;
    eprintln!("旧宿主兼容通过：wait code3，Future polls=0");
    Ok(())
}

async fn call(object: &RemoteObject, method: &str) -> std::result::Result<Value, WireError> {
    let request = Value::serialized(&payload::Invoke {
        object: object.id(),
        method: method.into(),
        args: Vec::new(),
        refs: Vec::new(),
    })
    .map_err(|error| WireError::new(2, error.to_string()))?;
    let response = object
        .peer()
        .request("invoke", request, Duration::from_secs(1))
        .await?;
    let result: payload::InvokeResult = response
        .value()
        .deserialized()
        .map_err(|error| WireError::new(2, error.to_string()))?;
    Ok(result.value)
}
