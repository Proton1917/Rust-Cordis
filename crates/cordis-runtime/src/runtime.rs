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
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    signal::unix::{Signal, SignalKind, signal},
    sync::{mpsc, watch},
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
    configuration
        .check_version(&semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(parameter)?)?;
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
    let locations = Locations::create(project, &configuration)?;
    let store = Store::create(config::resolve_path(
        project,
        &configuration.runtime.state_dir,
    ))?;
    let overlay = store.overlay()?;
    let settings = &configuration.runtime;
    let options = ManagerOptions {
        apply_timeout: Duration::from_millis(settings.apply_timeout_ms),
        cleanup_timeout: Duration::from_millis(settings.cleanup_timeout_ms),
        export_timeout: Duration::from_millis(settings.export_timeout_ms),
        invoke_timeout: Duration::from_millis(settings.invoke_timeout_ms),
        swap_timeout: Duration::from_millis(settings.swap_timeout_ms),
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
    let permissions = Arc::new(Mutex::new(BTreeMap::new()));
    let (handle, requests) = Handle::channel(Duration::from_millis(settings.swap_timeout_ms));
    let controller = Controller::new(
        manager,
        configuration,
        path,
        locations,
        store,
        overlay,
        ControlServices {
            diagnostics,
            permissions,
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
        "cordis.runtime@1.4",
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
    let mut writer = StatusWriter::start(view, controller.store.directory.join("status.json"));
    if let Some(session) = session {
        let result = tokio::select! {
            result = session.run(&mut controller) => result,
            _ = terminate.recv() => Err(WireError::new(7, "契约会话收到终止信号")),
            _ = interrupt.recv() => Err(WireError::new(7, "契约会话收到中断信号")),
            result = &mut writer.task => writer.observed(result).and(Err(WireError::new(7, "契约会话状态发布任务已结束"))),
        };
        return finish(&controller, &mut requests, writer, result.map(|_| None)).await;
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
    finish(&controller, &mut requests, writer, outcome).await
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
    loop {
        tokio::select! {
            _ = terminate.recv() => return Ok(None),
            _ = interrupt.recv() => return Ok(None),
            result = &mut writer.task => return writer.observed(result).map(|_| None),
            _ = checkpoint.tick(), if interval_ms > 0 => {
                let result = controller.manager.checkpoint().await.and_then(|states| controller.store.checkpoint(&states));
                if let Err(problem) = result { controller.logs.emit(0, "runtime", &format!("检查点写入失败：{problem}")); }
            }
            request = requests.recv() => {
                let request = request.ok_or_else(|| WireError::new(7, "管理队列已结束"))?;
                if let Some(request) = dispatch(controller, request).await { return Ok(Some(request)); }
            }
        }
    }
}

/// DOC-15 §3.2：普通请求返回实际结果，shutdown 的最终审计在资源释放后记录。
async fn dispatch(controller: &mut Controller, request: Request) -> Option<Request> {
    let result = controller.execute(&request).await;
    if matches!(request.operation, Operation::Shutdown) && result.is_ok() {
        return Some(request);
    }
    controller.audit(&request, &result);
    if let Some(reply) = request.reply {
        let _receiver = reply.send(result);
    }
    None
}

/// DOC-11 §9、CP-7、CP-11：停止排队、导出最终状态、结束所有分组，再发布最终阶段。
async fn finish(
    controller: &Controller,
    requests: &mut mpsc::Receiver<Request>,
    writer: StatusWriter,
    outcome: Result<Option<Request>>,
) -> Result<()> {
    controller.handle.stop();
    requests.close();
    while let Ok(request) = requests.try_recv() {
        if let Some(reply) = request.reply {
            let _receiver = reply.send(Err(WireError::new(7, "核心退出，排队操作已取消")));
        }
    }
    controller.diagnostics.lock().phase = "stopping".into();
    let exported = controller
        .manager
        .export_shutdown()
        .await
        .and_then(|states| controller.store.checkpoint(&states));
    let shutdown = controller.manager.shutdown().await;
    let (outcome, request) = match outcome {
        Ok(request) => (Ok(()), request),
        Err(problem) => (Err(problem), None),
    };
    let stopped = outcome.and(exported).and(shutdown);
    if let Some(request) = request {
        controller.audit(&request, &stopped.clone().map(|_| cordis_up::Value::Null));
    }
    {
        let mut diagnostics = controller.diagnostics.lock();
        diagnostics.phase = if stopped.is_ok() {
            "stopped"
        } else {
            "shutdown-failed"
        }
        .into();
        diagnostics.desired.clear();
    }
    let written = writer.finish().await;
    stopped.and(written)
}

struct StatusWriter {
    stop: watch::Sender<bool>,
    task: JoinHandle<Result<()>>,
    result: Option<Result<()>>,
}

impl StatusWriter {
    /// DOC-15 CP-11：停止标记在每次写入前读取，标记到达后写出最终快照再结束。
    fn start(view: View, path: PathBuf) -> Self {
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut previous = Vec::new();
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                let finishing = *stopped.borrow_and_update();
                status::write(&path, &view, &mut previous).await?;
                if finishing {
                    return Ok(());
                }
                tokio::select! { _ = interval.tick() => {}, result = stopped.changed() => {
                    if result.is_err() { return Ok(()); }
                } }
            }
        });
        Self {
            stop,
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
