use super::*;
use cordis_supervisor::manager::RetiredImageBasis;

pub(super) async fn run(artifacts: &Artifacts) -> Result<()> {
    released_handles(artifacts).await?;
    retained_handles(artifacts, RetiredImageBasis::OpenHandles).await?;
    retained_handles(artifacts, RetiredImageBasis::MappedImages).await
}

async fn released_handles(artifacts: &Artifacts) -> Result<()> {
    let manager = Manager::new(
        Context::new(Arc::new(Executor(Instant::now()))),
        ManagerOptions {
            retired_image_limit: 0,
            retired_image_basis: RetiredImageBasis::OpenHandles,
            confirm: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .await?;
    let result: Result<()> = async {
        let name = "retirement-metrics";
        let original = manager
            .attach_group(group(
                name,
                &artifacts.work.join(name),
                artifacts,
                &[&artifacts.provider],
            )?)
            .await?;
        manager
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
            .await?
            .settled()
            .await?;
        manager
            .invoke_service("loader.counter@1.0", "add", vec![37u64.into()])
            .await?;
        for round in 1..=3 {
            let report = manager
                .replace_library(
                    name.into(),
                    "loader-fixture".into(),
                    artifacts.version2.clone(),
                )
                .await?;
            check(report.succeeded, "native replacement completed")?;
            let status = manager
                .library_retirement(name)
                .await?
                .ok_or_else(|| std::io::Error::other("retirement metrics missing"))?;
            check(
                status.retired_total >= round && status.closed_total >= round,
                "retirement and close counters advance",
            )?;
            check(
                status.open_retired_handles == 0 && status.retired_leases == 0,
                "retired handles and actual execution leases are released",
            )?;
            check(
                manager.snapshot().groups[0].connection == original.connection,
                "open-handle policy preserves the host after successful close",
            )?;
            check(
                manager
                    .invoke_service("loader.counter@1.0", "value", Vec::new())
                    .await?
                    == Value::from(137u64),
                "replacement preserves actual business state",
            )?;
            #[cfg(target_os = "linux")]
            check(
                status.mapped_retired_images.is_some(),
                "Linux reports an actual mapping observation",
            )?;
            eprintln!("retirement round {round}: {status:?}");
        }
        Ok(())
    }
    .await;
    let stopped = manager.shutdown().await;
    result?;
    Ok(stopped?)
}

async fn retained_handles(artifacts: &Artifacts, basis: RetiredImageBasis) -> Result<()> {
    let manager = Manager::new(
        Context::new(Arc::new(Executor(Instant::now()))),
        ManagerOptions {
            retired_image_limit: 1,
            retired_image_basis: basis,
            confirm: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .await?;
    let result: Result<()> = async {
        let name = if basis == RetiredImageBasis::OpenHandles {
            "retirement-never-handles"
        } else {
            "retirement-never-mapped"
        };
        let mut specification = group(
            name,
            &artifacts.work.join(name),
            artifacts,
            &[&artifacts.provider],
        )?;
        let unload = specification
            .command
            .iter()
            .position(|argument| argument == "--unload")
            .unwrap()
            + 1;
        specification.command[unload] = "never".into();
        let original = manager.attach_group(specification).await?;
        manager
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
            .await?
            .settled()
            .await?;
        manager
            .invoke_service("loader.counter@1.0", "add", vec![37u64.into()])
            .await?;
        for round in 1..=2 {
            let report = manager
                .replace_library(
                    name.into(),
                    "loader-fixture".into(),
                    artifacts.version2.clone(),
                )
                .await?;
            check(report.succeeded, "Never replacement completed")?;
            let status = manager.library_retirement(name).await?.unwrap();
            let current = manager.snapshot().groups[0].connection;
            if round == 1 {
                check(
                    status.retired_total == 1
                        && status.closed_total == 0
                        && status.open_retired_handles == 1
                        && status.retired_leases == 0
                        && status.retired_cache_pins == 0,
                    "Never retains the image handle after code and cache leases are released",
                )?;
                #[cfg(target_os = "linux")]
                check(
                    status.mapped_retired_images == Some(1),
                    "Never retains one observed retired image",
                )?;
                check(
                    current == original.connection,
                    "one retained image is within the limit",
                )?;
            } else {
                check(
                    current != original.connection && status.retired_total == 0,
                    "second retained handle exceeds the limit and replaces the host",
                )?;
            }
            check(
                manager
                    .invoke_service("loader.counter@1.0", "value", Vec::new())
                    .await?
                    == Value::from(137u64),
                "Never recovery preserves business state",
            )?;
            eprintln!("Never {basis:?} retirement round {round}: {status:?}");
        }
        Ok(())
    }
    .await;
    let stopped = manager.shutdown().await;
    result?;
    Ok(stopped?)
}
