//! ADR-0055：真实原生宿主中，插件组件直接使用插件库的 tokio 运行时。
//!
//! 使用方动态库由 Rust 1.85.0 编译，带有自己的一份 tokio；宿主的 tokio 运行时对它不可见。

use super::*;

pub(super) async fn run(artifacts: &Artifacts) -> Result<()> {
    let manager = Manager::new(
        Context::new(Arc::new(Executor(Instant::now()))),
        ManagerOptions {
            confirm: Duration::from_millis(10),
            ..ManagerOptions::default()
        },
    )
    .await?;
    let operation = async {
        manager
            .attach_group(group(
                "tokio-context",
                &artifacts.work.join("tokio-context"),
                artifacts,
                &[&artifacts.consumer],
            )?)
            .await?;
        let fiber = manager
            .mount(
                InstanceSpec {
                    name: "tokio-probe".into(),
                    group: "tokio-context".into(),
                    plugin: "loader-consumer".into(),
                    component: "tokio-probe".into(),
                    config: Value::Map(Vec::new()),
                },
                None,
            )
            .await?;
        fiber.settled().await?;
        check(
            fiber.state() == FiberState::Active,
            "组件在 apply 中使用 tokio::spawn 与 tokio::time 后进入 Active",
        )
    };
    let result = tokio::time::timeout(Duration::from_secs(10), operation).await;
    let cleanup = tokio::time::timeout(Duration::from_secs(10), manager.shutdown()).await;
    result.map_err(|_| std::io::Error::other("tokio 上下文场景超过10秒"))??;
    cleanup.map_err(|_| std::io::Error::other("tokio 上下文场景回收超过10秒"))??;
    Ok(())
}
