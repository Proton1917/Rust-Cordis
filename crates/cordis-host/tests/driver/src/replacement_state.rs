//! ADR-0052：原生候选确认期退出后恢复原库与跨宿主依赖方快照。

use super::*;

pub(super) async fn run(artifacts: &Artifacts) -> Result<()> {
    let manager = Manager::new(
        Context::new(Arc::new(Executor(Instant::now()))),
        ManagerOptions {
            confirm: Duration::from_secs(2),
            ..Default::default()
        },
    )
    .await?;
    let result = tokio::time::timeout(Duration::from_secs(20), exercise(&manager, artifacts)).await;
    let cleanup = manager.shutdown().await;
    result.map_err(|_| std::io::Error::other("确认期回滚场景超过20秒"))??;
    cleanup?;
    Ok(())
}

async fn prepare(manager: &Manager, artifacts: &Artifacts) -> Result<Fiber> {
    for (name, library, plugin, component) in [
        (
            "upstream",
            &artifacts.provider,
            "loader-fixture",
            "provider",
        ),
        ("downstream", &artifacts.consumer, "loader-consumer", "low"),
    ] {
        manager
            .attach_group(group(
                name,
                &artifacts.work.join(name),
                artifacts,
                &[library],
            )?)
            .await?;
        manager
            .mount(
                InstanceSpec {
                    name: name.into(),
                    group: name.into(),
                    plugin: plugin.into(),
                    component: component.into(),
                    config: Value::Map(Vec::new()),
                },
                None,
            )
            .await?
            .settled()
            .await?;
    }
    manager
        .invoke_service("host.downstream@1.0", "add", vec![37u64.into()])
        .await?;
    Ok(manager.instance_fiber("upstream")?)
}

async fn exercise(manager: &Manager, artifacts: &Artifacts) -> Result<()> {
    let provider = prepare(manager, artifacts).await?;
    let old_epoch = provider.epoch();
    let changing = manager.clone();
    let path = artifacts.version2.clone();
    let replacement = tokio::spawn(async move {
        changing
            .replace_library("upstream".into(), "loader-fixture".into(), path)
            .await
    });
    loop {
        let consumer = manager.instance_fiber("downstream")?;
        if provider.epoch() != old_epoch
            && provider.is_settled()
            && consumer.is_settled()
            && provider.state() == FiberState::Active
            && consumer.state() == FiberState::Active
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let written = manager
        .invoke_service("host.downstream@1.0", "add", vec![5u64.into()])
        .await?;
    check(
        written == Value::Integer(142u64.into()),
        "确认期写入已应答142",
    )?;
    let pid = manager
        .snapshot()
        .groups
        .iter()
        .find(|group| group.name == "upstream")
        .and_then(|group| group.pid)
        .ok_or_else(|| std::io::Error::other("上游宿主PID缺席"))?;
    check(crate::force_kill(pid)?, "终止候选宿主")?;
    check(replacement.await?.is_err(), "候选宿主退出使替换失败")?;
    let restored = manager
        .invoke_service("host.downstream@1.0", "add", vec![0u64.into()])
        .await?;
    check(
        restored == Value::Integer(137u64.into()),
        "回滚恢复原依赖方137",
    )?;
    let version = manager
        .invoke_service("loader.counter@1.0", "implementation", Vec::new())
        .await?;
    check(version == Value::Integer(1u64.into()), "原宿主计划恢复v1")?;
    let report = manager
        .library_swap_reports()
        .into_iter()
        .find(|(_, r)| r.plugin == "loader-fixture")
        .map(|(_, report)| report)
        .ok_or_else(|| std::io::Error::other("换库报告缺席"))?;
    check(report.rolled_back, "报告确认原生回滚完成")?;
    let discarded = report
        .discarded_confirmation
        .ok_or_else(|| std::io::Error::other("确认期丢弃报告缺席"))?;
    check(
        discarded.instances == ["downstream", "upstream"] && discarded.configured_ms == 2000,
        "丢弃报告覆盖原生提供方及跨宿主依赖方",
    )?;
    eprintln!("原生确认期：写入142已应答，宿主退出后恢复137，丢弃报告包含两实例和2000ms窗口");
    Ok(())
}
