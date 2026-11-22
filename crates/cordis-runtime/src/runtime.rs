//! DOC-15 Q0、CP-7、CP-11：核心启动、管理事件、检查点与最终状态发布。

use crate::{
    builtins::{DataDirectory, Logs, RuntimeInfo},
    config::{self, Configuration, Result, parameter},
    contract::{Plan, Session},
    control::{ControlServices, Controller},
    deployment::Locations,
    executor::Executor,
    request::{Handle, Operation, Request},
    status::{self, Diagnostics, View},
    storage::{Store, io_error},
};
use cordis::{Context, ExporterOptions, LogLevel, LogValue};
use cordis_supervisor::manager::{Manager, ManagerOptions};
use cordis_up::WireError;
use parking_lot::Mutex;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    signal::unix::{Signal, SignalKind, signal},
    sync::{Notify, mpsc, watch},
    task::{JoinError, JoinHandle},
    time::{Instant, MissedTickBehavior},
};

/// DOC-15 §3.1、CP-7：配置通过版本检查后创建唯一根，所有结束路径释放该根。
pub async fn run(path: PathBuf) -> Result<()> {
    run_with_contract(path, None).await
}

/// DOC-11 §8：验证会话与普通部署共用唯一内核，最终报告在完整清理之后写出。
pub async fn run_with_contract(path: PathBuf, plan: Option<Plan>) -> Result<()> {
    let session = plan.map(Session::new).transpose()?;
    let path = path.canonicalize().map_err(io_error)?;
    let configuration = crate::deployment::load(&path)?;
    let directory = config::resolve_path(
        path.parent().expect("configuration parent"),
        &configuration.runtime.state_dir,
    );
    let _owner = crate::storage::claim(&directory)?;
    let mut terminate = signal(SignalKind::terminate()).map_err(io_error)?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(io_error)?;
    let root = Context::new(Arc::new(Executor::new()));
    let result = async {
        let (controller, requests, view) = create_controller(&root, path, configuration).await?;
        if let Some(session) = &session {
            session.install(&controller.manager)?;
        }
        serve(
            controller,
            requests,
            view,
            &mut terminate,
            &mut interrupt,
            session.as_ref(),
        )
        .await
    }
    .await;
    let disposed = root.fiber().dispose().await.map_err(kernel);
    let result = result.and(disposed);
    if let Some(session) = session {
        session.save(&result)?;
    }
    result
}

/// DOC-15 §3.2、CP-11：创建状态目录与监管器，并把内置服务和授权队列交给控制器。
async fn create_controller(
    root: &Context,
    path: PathBuf,
    configuration: Configuration,
) -> Result<(Controller, mpsc::Receiver<Request>, View)> {
    let project = path.parent().expect("configuration parent");
    let store = Store::create(config::resolve_path(
        project,
        &configuration.runtime.state_dir,
    ))?;
    store.recover_deployment()?;
    let configuration = store.accepted_base()?.unwrap_or(configuration);
    configuration
        .check_version(&semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(parameter)?)?;
    let locations = Locations::create(project, &configuration)?;
    let overlay = store.overlay()?;
    let settings = &configuration.runtime;
    let lifecycle = Duration::from_millis(settings.lifecycle_timeout_ms);
    let options = ManagerOptions {
        apply_timeout: lifecycle,
        cleanup_timeout: lifecycle,
        export_timeout: lifecycle,
        invoke_timeout: Duration::from_millis(settings.invoke_timeout_ms),
        swap_timeout: lifecycle,
        confirm: Duration::from_millis(settings.confirm_ms),
        retired_image_limit: settings.retired_image_limit as u64,
        ..ManagerOptions::default()
    };
    let manager = Arc::new(Manager::new(root.clone(), options).await?);
    let diagnostics = Arc::new(Mutex::new(Diagnostics {
        phase: "starting".into(),
        ..Diagnostics::default()
    }));
    let view = View::new(&manager, diagnostics.clone());
    let logs = register_services(root, &manager, &store, &view)?;
    // 管理请求的等待属于调用时限，与生命周期操作的时限分开。
    let (handle, requests) = Handle::channel(Duration::from_millis(settings.invoke_timeout_ms));
    let controller = Controller::new(
        manager,
        configuration,
        path,
        locations,
        store,
        overlay,
        ControlServices {
            diagnostics,
            handle,
            logs,
        },
    )?;
    Ok((controller, requests, view))
}

/// ADR-0017、CP-5：注册纯数据内置接口，日志在内核锁外转发给插件。
fn register_services(
    root: &Context,
    manager: &Arc<Manager>,
    store: &Store,
    view: &View,
) -> Result<Logs> {
    let logs = Logs(Arc::downgrade(manager));
    let sink = logs.clone();
    manager.set_log_sink(Arc::new(move |record| sink.record(record)));
    manager.provide_builtin(
        "cordis.data-dir@1.0",
        vec!["path".into()],
        Arc::new(DataDirectory(store.clone())),
    )?;
    manager.provide_builtin(
        "cordis.runtime@1.5",
        vec!["snapshot".into()],
        Arc::new(RuntimeInfo(view.clone())),
    )?;
    let exported = logs.clone();
    root.logger()
        .exporter(
            ExporterOptions {
                default_level: Some(LogLevel::Debug),
                ..ExporterOptions::default()
            },
            move |record| {
                let message = record
                    .args
                    .iter()
                    .map(|value| match value {
                        LogValue::Text(text) => text.clone(),
                        LogValue::Error(problem) => problem.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let level = match record.level {
                    LogLevel::Error => 0,
                    LogLevel::Warn => 1,
                    LogLevel::Info => 2,
                    LogLevel::Debug => 3,
                };
                exported.emit(level, &record.name, &message);
            },
        )
        .map_err(kernel)?;
    Ok(logs)
}

/// DOC-15 CP-11：状态发布与管理调度独立进行，启动失败也执行完整退出步骤。
async fn serve(
    mut controller: Controller,
    mut requests: mpsc::Receiver<Request>,
    view: View,
    terminate: &mut Signal,
    interrupt: &mut Signal,
    session: Option<&Session>,
) -> Result<()> {
    let mut writer = StatusWriter::start(
        view,
        controller.store.directory.join("status.json"),
        controller.logs.clone(),
    );
    if let Some(session) = session {
        let result = tokio::select! {
            result = session.run(&mut controller) => result,
            _ = terminate.recv() => Err(WireError::new(7, "契约会话收到终止信号")),
            _ = interrupt.recv() => Err(WireError::new(7, "契约会话收到中断信号")),
            result = &mut writer.task => writer.observed(result).and(Err(WireError::new(7, "契约会话状态发布任务已结束"))),
        };
        return finish(&mut controller, &mut requests, writer, result.map(|_| None)).await;
    }
    let outcome = match controller.start().await {
        Ok(()) => {
            controller.diagnostics.lock().phase = "running".into();
            controller.logs.emit(2, "runtime", "运行时已启动");
            dispatch_loop(
                &mut controller,
                &mut requests,
                &mut writer,
                terminate,
                interrupt,
            )
            .await
        }
        Err(problem) => Err(problem),
    };
    finish(&mut controller, &mut requests, writer, outcome).await
}

/// DOC-15 §3.2、CP-11：管理事务串行执行，检查点失败记录后继续服务。
async fn dispatch_loop(
    controller: &mut Controller,
    requests: &mut mpsc::Receiver<Request>,
    writer: &mut StatusWriter,
    terminate: &mut Signal,
    interrupt: &mut Signal,
) -> Result<Option<Request>> {
    let interval_ms = controller.configuration.runtime.checkpoint_ms;
    let period = Duration::from_millis(interval_ms.max(1));
    let mut checkpoint = tokio::time::interval_at(Instant::now() + period, period);
    checkpoint.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut pending = Vec::new();
    let activated = Arc::new(Notify::new());
    let signal = activated.clone();
    controller
        .manager
        .context()
        .on::<cordis::InternalStatus>(move |(fiber, _)| {
            if fiber.state() == cordis::FiberState::Active {
                signal.notify_one();
            }
            None
        })
        .map_err(kernel)?;
    // 监听登记前已激活的实例也在首次确认范围内。
    activated.notify_one();
    let mut source_retry = None;
    let handle = controller.handle.clone();
    let mut admission = handle.admission();
    let diagnostics = controller.diagnostics.clone();
    loop {
        enum Work {
            Checkpoint(String),
            Manage(Request),
        }
        let work = tokio::select! {
            biased;
            _ = terminate.recv() => return Ok(None),
            _ = interrupt.recv() => return Ok(None),
            _ = admission.wait_for(|open| !*open) => return Ok(None),
            result = &mut writer.task => return writer.observed(result).map(|_| None),
            request = requests.recv() => {
                Work::Manage(request.ok_or_else(|| WireError::new(7, "管理队列已结束"))?)
            }
            _ = async {
                match source_retry {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => activated.notified().await,
                }
            } => {
                source_retry = controller.confirm_sources().err().map(|problem| {
                    controller.logs.emit(0, "runtime", &format!("代码来源保存失败：{problem}"));
                    Instant::now() + Duration::from_secs(1)
                });
                continue;
            }
            _ = checkpoint.tick(), if interval_ms > 0 && pending.is_empty() && controller.recovery_required.is_none() => {
                pending = controller.manager.deployment_instance_names();
                continue;
            }
            _ = std::future::ready(()), if !pending.is_empty() && controller.recovery_required.is_none() => {
                Work::Checkpoint(pending.pop().expect("检查点队列持有当前实例"))
            }
        };
        let executing = async {
            match work {
                Work::Checkpoint(name) => {
                    let result = controller
                        .manager
                        .checkpoint_instance(name)
                        .await
                        .and_then(|states| controller.store.checkpoint(&states));
                    if let Err(problem) = result {
                        controller
                            .logs
                            .emit(0, "runtime", &format!("检查点写入失败：{problem}"));
                    }
                    None
                }
                Work::Manage(request) => {
                    let finished = dispatch(controller, request).await;
                    writer.refresh.notify_one();
                    finished
                }
            }
        };
        tokio::pin!(executing);
        tokio::select! {
            finished = &mut executing => {
                if finished.is_some() { return Ok(finished); }
                continue;
            },
            _ = terminate.recv() => {},
            _ = interrupt.recv() => {},
            _ = admission.wait_for(|open| !*open) => {},
        }
        // 先停止受理，再等待队列所有者结束已经开始的导出或部署。
        handle.stop();
        diagnostics.lock().phase = "stopping".into();
        writer.refresh.notify_one();
        return Ok(executing.await);
    }
}

/// DOC-15 §3.2：普通请求返回实际结果，shutdown 的最终审计在资源释放后记录。
async fn dispatch(controller: &mut Controller, mut request: Request) -> Option<Request> {
    let result = controller.execute(&request).await;
    if matches!(request.operation, Operation::Shutdown) && result.is_ok() {
        return Some(request);
    }
    controller.audit(&request, &result);
    if let Some(ticket) = request.pending_batch.take() {
        let cleanup = controller.last_cleanup_error.take();
        let restored = controller.batch_restored.take();
        let (state, problem) = if let Some(problem) = &controller.recovery_required {
            ("recovery_required", Some(problem))
        } else if let Err(problem) = &result {
            (
                if restored.is_some() {
                    "restored"
                } else {
                    "rejected"
                },
                Some(problem),
            )
        } else if let Some(problem) = &cleanup {
            ("committed_with_cleanup", Some(problem))
        } else {
            ("succeeded", None)
        };
        ticket.finish(state, problem, restored.flatten());
    }
    if let Some(reply) = request.reply {
        let _receiver = reply.send(result);
    }
    None
}

/// DOC-11 §9、CP-7、CP-11：停止排队、导出最终状态、结束所有分组，再发布最终阶段。
async fn finish(
    controller: &mut Controller,
    requests: &mut mpsc::Receiver<Request>,
    writer: StatusWriter,
    outcome: Result<Option<Request>>,
) -> Result<()> {
    controller.handle.stop();
    requests.close();
    let (outcome, mut stop_request) = match outcome {
        Ok(request) => (Ok(()), request),
        Err(problem) => (Err(problem), None),
    };
    while let Ok(request) = requests.try_recv() {
        if matches!(request.operation, Operation::Shutdown) && stop_request.is_none() {
            stop_request = Some(request);
        } else if let Some(reply) = request.reply {
            let _receiver = reply.send(Err(WireError::new(7, "核心退出，排队操作已取消")));
        }
    }
    controller.diagnostics.lock().phase = "stopping".into();
    let exported = match &controller.recovery_required {
        Some(problem) => Err(problem.clone()),
        None => controller
            .manager
            .export_shutdown()
            .await
            .and_then(|states| controller.store.checkpoint(&states)),
    };
    // 此时实例仍在目录中，停机后的离线查询可取得最后一次 Active 绑定。
    let sources = controller.confirm_sources();
    let shutdown = controller.manager.shutdown().await;
    let stopped = outcome.and(exported).and(sources).and(shutdown);
    if let Some(mut request) = stop_request {
        controller.audit(&request, &stopped.clone().map(|_| cordis_up::Value::Null));
        if let Some(ticket) = request.pending_batch.take() {
            let state = if stopped.is_ok() {
                "succeeded"
            } else {
                "rejected"
            };
            ticket.finish(state, stopped.as_ref().err(), None);
        }
    }
    {
        let mut diagnostics = controller.diagnostics.lock();
        diagnostics.outcomes.lock().end_pending();
        diagnostics.phase = if stopped.is_ok() {
            "stopped"
        } else {
            "shutdown-failed"
        }
        .into();
        diagnostics.desired = Arc::default();
    }
    let written = writer.finish().await;
    stopped.and(written)
}

/// 状态文件的兜底刷新周期。每次刷新都经 UP/1 查询所有原生宿主的库状态，
/// 周期过短会让空闲部署持续产生跨进程调用；管理请求完成后另有即时刷新。
const STATUS_PERIOD: Duration = Duration::from_secs(1);

struct StatusWriter {
    stop: watch::Sender<bool>,
    refresh: Arc<Notify>,
    task: JoinHandle<Result<()>>,
    result: Option<Result<()>>,
}

impl StatusWriter {
    /// DOC-15 CP-11：停止标记在每次写入前读取，标记到达后写出最终快照再结束。
    /// 周期刷新覆盖异步变化（崩溃恢复、宿主状态），`refresh` 让管理请求的结果立即可见。
    fn start(view: View, path: PathBuf, logs: Logs) -> Self {
        let (stop, mut stopped) = watch::channel(false);
        let refresh = Arc::new(Notify::new());
        let requested = refresh.clone();
        let task = tokio::spawn(async move {
            let mut previous = Vec::new();
            let mut interval = tokio::time::interval(STATUS_PERIOD);
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                let finishing = *stopped.borrow_and_update();
                let published = status::write(&path, &view, &mut previous).await;
                if finishing {
                    return published;
                }
                if let Err(problem) = published {
                    logs.emit(0, "runtime", &format!("状态快照发布失败：{problem}"));
                }
                tokio::select! {
                    _ = interval.tick() => {},
                    _ = requested.notified() => interval.reset(),
                    result = stopped.changed() => {
                        if result.is_err() { return Ok(()); }
                    }
                }
            }
        });
        Self {
            stop,
            refresh,
            task,
            result: None,
        }
    }

    fn observed(&mut self, result: std::result::Result<Result<()>, JoinError>) -> Result<()> {
        let result = result
            .map_err(|problem| WireError::new(1, problem.to_string()))
            .and_then(|result| result);
        self.result = Some(result.clone());
        result
    }

    async fn finish(self) -> Result<()> {
        self.stop.send_replace(true);
        match self.result {
            Some(result) => result,
            None => self
                .task
                .await
                .map_err(|problem| WireError::new(1, problem.to_string()))?,
        }
    }
}

fn kernel(problem: cordis::Error) -> WireError {
    WireError::new(1, problem.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_testkit::cleanup::TestGuard;
    use std::{fs, path::Path};

    #[tokio::test]
    async fn status_writer_recovers_from_file_error() {
        let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "status-writer");
        let directory = case.directory("status-writer");
        fs::create_dir_all(directory.join("status.json.next")).unwrap();
        let root = Context::new(Arc::new(Executor::new()));
        let manager = Arc::new(
            Manager::new(root.clone(), ManagerOptions::default())
                .await
                .unwrap(),
        );
        let diagnostics = Arc::new(Mutex::new(Diagnostics::default()));
        let view = View::new(&manager, diagnostics);
        let writer = StatusWriter::start(
            view,
            directory.join("status.json"),
            Logs(Arc::downgrade(&manager)),
        );
        tokio::task::yield_now().await;
        assert!(!writer.task.is_finished(), "状态写入失败后继续运行发布任务");
        fs::remove_dir(directory.join("status.json.next")).unwrap();
        writer.refresh.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !directory.join("status.json").is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        writer.finish().await.unwrap();
        manager.shutdown().await.unwrap();
        root.fiber().dispose().await.unwrap();
        case.success();
    }
}
