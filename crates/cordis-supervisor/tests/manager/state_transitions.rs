//! ADR-0052 I-1：真实进程中的有状态依赖方随上游替换继承最新值。

use super::*;

async fn stateful_pair(
    manager: &Manager,
    control: &Control,
    restart: RestartPolicy,
) -> (Ready, Ready) {
    manager
        .attach_group(group("upstream", "v1", "upstream-fixture", restart))
        .await
        .unwrap();
    manager
        .attach_group(group(
            "downstream",
            "v1",
            "downstream-fixture",
            no_restart(),
        ))
        .await
        .unwrap();
    for (name, plugin, component, initial) in [
        ("upstream", "upstream-fixture", "b", 29),
        ("downstream", "downstream-fixture", "stateful", 100),
    ] {
        manager
            .mount(
                instance(name, name, plugin, component, configuration(initial)),
                None,
            )
            .await
            .unwrap()
            .settled()
            .await
            .unwrap();
    }
    (
        control.ready_after("upstream", 0).await,
        control.ready_after("downstream", 0).await,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upstream_group_replacement_preserves_latest_downstream_state() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "dependent-state");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    let (_, old) = stateful_pair(&manager, &control, no_restart()).await;
    let written = integer(&invoke(&old, "add", vec![37u64.into()]).await);
    let saved = integer(&manager.checkpoint().await.unwrap()["downstream"].data);
    manager
        .replace_group(group("upstream", "v2", "upstream-fixture", no_restart()))
        .await
        .unwrap();
    let current = control.ready_after("downstream", old.context).await;
    let restored = integer(&invoke(&current, "get", Vec::new()).await);
    manager.shutdown().await.unwrap();
    assert_eq!([written, saved, restored], [137, 137, 137]);
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upstream_configuration_update_preserves_downstream_state() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "dependent-update");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    let (_, old) = stateful_pair(&manager, &control, no_restart()).await;
    invoke(&old, "add", vec![37u64.into()]).await;
    manager.update("upstream", configuration(99)).await.unwrap();
    let next = control.ready_after("downstream", old.context).await;
    let restored = integer(&invoke(&next, "get", Vec::new()).await);
    manager.shutdown().await.unwrap();
    assert_eq!(restored, 137);
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_update_releases_optional_consumers_original_exporter() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "optional-update");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    manager
        .attach_group(group("pair", "v1", "pair", no_restart()))
        .await
        .unwrap();
    let file = case.directory("optional-update").join("service");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "off").unwrap();
    let config =
        |path: &Path| Value::Map(vec![("service_file".into(), path.to_str().unwrap().into())]);
    manager
        .mount(
            instance("provider", "pair", "pair", "b", config(&file)),
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
                "reader",
                "pair",
                "pair",
                "optional-stateful",
                configuration(100),
            ),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let reader = control.ready_after("reader", 0).await;
    assert!(
        manager
            .update("provider", config(&file.with_extension("missing")))
            .await
            .is_err()
    );
    let written = integer(&invoke(&reader, "add", vec![7u64.into()]).await);
    manager.shutdown().await.unwrap();
    assert_eq!(written, 107);
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_replacement_preserves_implicit_downstream_state() {
    use cordis_supervisor::manager::{BatchInstance, BatchReplacement};
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "dependent-batch");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    let (_, old) = stateful_pair(&manager, &control, no_restart()).await;
    invoke(&old, "add", vec![37u64.into()]).await;
    let before: Vec<_> = [
        ("upstream", "upstream-fixture", "b", 29),
        ("downstream", "downstream-fixture", "stateful", 100),
    ]
    .into_iter()
    .map(|(name, plugin, component, initial)| BatchInstance {
        specification: instance(name, name, plugin, component, configuration(initial)),
        version: "0.1.0".into(),
        view: manager.context(),
        remount: false,
    })
    .collect();
    let mut target = before.clone();
    target
        .iter_mut()
        .find(|item| item.specification.name == "upstream")
        .unwrap()
        .version = "0.2.0".into();
    manager
        .replace_batch(
            vec![BatchReplacement::Process(group(
                "upstream",
                "v2",
                "upstream-fixture",
                no_restart(),
            ))],
            before,
            target,
            HashMap::new(),
        )
        .await
        .unwrap();
    let next = control.ready_after("downstream", old.context).await;
    let restored = integer(&invoke(&next, "get", Vec::new()).await);
    manager.shutdown().await.unwrap();
    assert_eq!(restored, 137);
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crashed_provider_uses_checkpoint_and_preserves_live_downstream() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "dependent-crash");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    let (provider, consumer) = stateful_pair(
        &manager,
        &control,
        RestartPolicy {
            max: 1,
            backoff: Duration::from_millis(150),
        },
    )
    .await;
    invoke(&provider, "add", vec![5u64.into()]).await;
    invoke(&consumer, "add", vec![37u64.into()]).await;
    manager.checkpoint().await.unwrap();
    invoke(&provider, "add", vec![1u64.into()]).await;
    let pid = manager
        .snapshot()
        .groups
        .iter()
        .find(|g| g.name == "upstream")
        .unwrap()
        .pid
        .unwrap();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let provider = control.ready_after("upstream", provider.context).await;
    let consumer = control.ready_after("downstream", consumer.context).await;
    let restored = [
        integer(&invoke(&provider, "get", Vec::new()).await),
        integer(&invoke(&consumer, "get", Vec::new()).await),
    ];
    manager.shutdown().await.unwrap();
    assert_eq!(restored, [34, 137]);
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn optional_dependent_keeps_writes_during_provider_restart() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "optional-recovery");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    let (provider, _) = stateful_pair(
        &manager,
        &control,
        RestartPolicy {
            max: 1,
            backoff: Duration::from_secs(2),
        },
    )
    .await;
    manager.dispose("downstream").await.unwrap();
    manager
        .mount(
            instance(
                "optional",
                "downstream",
                "downstream-fixture",
                "optional-stateful",
                configuration(100),
            ),
            None,
        )
        .await
        .unwrap()
        .settled()
        .await
        .unwrap();
    let old = control.ready_after("optional", 0).await;
    invoke(&old, "add", vec![37u64.into()]).await;
    let pid = manager
        .snapshot()
        .groups
        .iter()
        .find(|group| group.name == "upstream")
        .unwrap()
        .pid
        .unwrap();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let waiting = control.ready_after("optional", old.context).await;
    // 同一管理队列中的检查点应在上游两秒退避期间完成。
    tokio::time::timeout(
        Duration::from_secs(1),
        manager.checkpoint_instances(&["optional".into()]),
    )
    .await
    .expect("其他实例可以在宿主退避期间保存检查点")
    .unwrap();
    let accepted = integer(&invoke(&waiting, "add", vec![5u64.into()]).await);
    control.ready_after("upstream", provider.context).await;
    let next = control.ready_after("optional", waiting.context).await;
    let restored = integer(&invoke(&next, "get", Vec::new()).await);
    manager.shutdown().await.unwrap();
    assert_eq!([accepted, restored], [142, 142]);
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn updating_provider_ends_inflight_call_before_deadline() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "inflight-update");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    stateful_pair(&manager, &control, no_restart()).await;
    let caller = manager.clone();
    let call = tokio::spawn(async move {
        caller
            .invoke_service("fixture.counter@1.0", "hold", Vec::new())
            .await
    });
    control.phase_after("upstream", "call-start", 0).await;
    let started = Instant::now();
    manager.update("upstream", configuration(99)).await.unwrap();
    let result = call.await.unwrap();
    let elapsed = started.elapsed();
    manager.shutdown().await.unwrap();
    eprintln!(
        "进行中调用：result={result:?} elapsed_ms={}",
        elapsed.as_millis()
    );
    assert!(matches!(result, Err(WireError { code: 4, .. })));
    assert!(
        elapsed < Duration::from_secs(4),
        "退役通知应早于 5 秒的调用超时结束调用"
    );
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confirmation_rollback_reports_discarded_dependent_state() {
    let case = TestGuard::for_package(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "confirmation-discard",
    );
    let _fixture = plugin().for_case(&case);
    let mut settings = options();
    settings.confirm = Duration::from_secs(2);
    let (manager, control) = setup(settings).await;
    let logs = Arc::new(Mutex::new(Vec::new()));
    let captured = logs.clone();
    manager.set_log_sink(Arc::new(move |record| captured.lock().push(record)));
    let (old_provider, old_consumer) = stateful_pair(&manager, &control, no_restart()).await;
    invoke(&old_consumer, "add", vec![37u64.into()]).await;
    let changing = manager.clone();
    let replacement = tokio::spawn(async move {
        changing
            .replace_group(group("upstream", "v2", "upstream-fixture", no_restart()))
            .await
    });
    control.ready_after("upstream", old_provider.context).await;
    let candidate_consumer = control
        .ready_after("downstream", old_consumer.context)
        .await;
    until(Duration::from_secs(5), || {
        ["upstream", "downstream"].iter().all(|name| {
            manager
                .instance_fiber(name)
                .is_ok_and(|fiber| fiber.state() == FiberState::Active && fiber.is_settled())
        })
    })
    .await;
    let accepted = integer(
        &manager
            .invoke_service("fixture.counter@1.0", "add", vec![100u64.into()])
            .await
            .unwrap(),
    );
    invoke(&candidate_consumer, "add", vec![5u64.into()]).await;
    let pid = manager
        .snapshot()
        .groups
        .iter()
        .find(|g| g.name == "upstream")
        .unwrap()
        .pid
        .unwrap();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let result = replacement.await.unwrap();
    let provider = control.ready_after("upstream", old_provider.context).await;
    let consumer = control
        .ready_after("downstream", candidate_consumer.context)
        .await;
    let restored = [
        integer(&invoke(&provider, "get", Vec::new()).await),
        integer(&invoke(&consumer, "get", Vec::new()).await),
    ];
    let report = manager
        .snapshot()
        .last_replace
        .into_iter()
        .find(|report| report.group == "upstream")
        .unwrap();
    manager.shutdown().await.unwrap();
    assert!(result.is_err());
    assert_eq!(accepted, 129);
    assert_eq!(restored, [29, 137]);
    assert!(report.rolled_back);
    let discarded = report.discarded_confirmation.unwrap();
    assert_eq!(discarded.instances, ["downstream", "upstream"]);
    assert_eq!(discarded.configured_ms, 2000);
    assert!(logs.lock().iter().any(
        |record| record.message.contains("确认期回滚") && record.message.contains("downstream")
    ));
    case.success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn updating_provider_interrupts_a_returned_byte_stream() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "stream-update");
    let _fixture = plugin().for_case(&case);
    let (manager, control) = setup(options()).await;
    let (_, consumer) = stateful_pair(&manager, &control, no_restart()).await;
    let bytes = vec![7, 8, 9];
    let stream = integer(
        &invoke(
            &consumer,
            "open-held-stream",
            vec![Value::Bytes(bytes.clone())],
        )
        .await,
    );
    let first = invoke(
        &consumer,
        "read-held-stream",
        vec![stream.into(), 64u64.into()],
    )
    .await;
    let call = tokio::spawn(async move {
        try_invoke(
            &consumer,
            "read-held-stream",
            vec![stream.into(), 64u64.into()],
        )
        .await
    });
    control
        .phase_after("upstream", "stream-read-start", 0)
        .await;
    let started = Instant::now();
    manager.update("upstream", configuration(99)).await.unwrap();
    let result = call.await.unwrap();
    let elapsed = started.elapsed();
    manager.shutdown().await.unwrap();
    eprintln!(
        "流读取：result={result:?} elapsed_ms={}",
        elapsed.as_millis()
    );
    assert_eq!(first, Value::Bytes(bytes));
    assert!(matches!(result, Err(WireError { code: 4, .. })));
    assert!(
        elapsed < Duration::from_secs(4),
        "退役通知应早于 5 秒的调用超时结束流读取"
    );
    case.success();
}
