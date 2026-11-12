use cordis_up::{
    Value, WireError,
    rpc::{Connection, ConnectionOptions, Handler},
};
use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use rustix::process::{WaitId, WaitIdOptions, waitid};
use std::{
    ops::Deref,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    signal::unix::{SignalKind, signal},
    sync::watch,
    time::timeout,
};

type Result<T> = std::result::Result<T, WireError>;
type Log = Arc<dyn Fn(u64, String) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct PeerOptions {
    pub rpc: ConnectionOptions,
    pub stderr_line_bytes: usize,
}
impl Default for PeerOptions {
    fn default() -> Self {
        Self {
            rpc: ConnectionOptions::default(),
            stderr_line_bytes: 16 * 1024,
        }
    }
}

struct Process {
    pid: u32,
    exited: watch::Receiver<Option<Result<String>>>,
    stop: watch::Sender<bool>,
    shutting_down: Arc<AtomicBool>,
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

#[derive(Clone)]
pub struct Peer {
    connection: Connection,
    process: Arc<Process>,
}
impl Deref for Peer {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl Peer {
    /// CP-7、ADR-0027：生命周期任务保有Child，先处理进程组，随后唯一回收PID。
    pub async fn launch(
        id: u64,
        command: &[String],
        cwd: &Path,
        options: PeerOptions,
        handler: Arc<dyn Handler>,
        log: Log,
    ) -> Result<Self> {
        if command.is_empty() || options.stderr_line_bytes == 0 {
            return Err(error(2, "进程命令与日志行容量需要有效取值"));
        }
        let mut children = signal(SignalKind::child()).map_err(io_error)?;
        let mut process = Command::new(&command[0]);
        process
            .args(&command[1..])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = process.spawn().map_err(io_error)?;
        let pid = child.id().expect("new child has PID");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let connection = match Connection::attach(id, stdout, stdin, options.rpc, handler) {
            Ok(connection) => connection,
            Err(problem) => {
                terminate_and_reap(&mut child, pid).await?;
                return Err(problem);
            }
        };
        let (exited, exit_status) = watch::channel(None);
        let (stop, mut stopping) = watch::channel(false);
        let shutting_down = Arc::new(AtomicBool::new(false));
        let process = Arc::new(Process {
            pid,
            exited: exit_status,
            stop,
            shutting_down: shutting_down.clone(),
        });
        let watched = connection.clone();
        let lifecycle_log = log.clone();
        tokio::spawn(async move {
            let result = async {
                loop {
                    if observe_exit(pid)? || *stopping.borrow() { break; }
                    tokio::select! {
                        _ = stopping.changed() => break,
                        received = children.recv() => {
                            if received.is_none() { return Err(error(4, "子进程退出信号流已关闭")); }
                        }
                        _ = watched.closed(), if !shutting_down.load(Ordering::Acquire) => {
                            if !shutting_down.load(Ordering::Acquire) { break; }
                        }
                    }
                }
                terminate_and_reap(&mut child, pid).await
            }.await;
            let reason = match &result {
                Ok(reason) => reason.clone(),
                Err(problem) => {
                    lifecycle_log(id, problem.to_string());
                    problem.to_string()
                }
            };
            watched.terminate(&reason);
            exited.send_replace(Some(result));
        });
        let watched = connection.clone();
        tokio::spawn(async move {
            let mut input = BufReader::new(stderr);
            let mut line = Vec::new();
            loop {
                let available = match input.fill_buf().await {
                    Ok(bytes) => bytes,
                    Err(problem) => {
                        watched.terminate(&problem.to_string());
                        break;
                    }
                };
                if available.is_empty() {
                    if !line.is_empty() {
                        log(id, String::from_utf8_lossy(&line).into_owned());
                    }
                    break;
                }
                let count = available
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(available.len(), |index| index + 1);
                if line.len() + count > options.stderr_line_bytes {
                    watched.terminate("插件标准错误行超出长度限制");
                    break;
                }
                line.extend_from_slice(&available[..count]);
                let complete = line.last() == Some(&b'\n');
                input.consume(count);
                if complete {
                    line.pop();
                    log(id, String::from_utf8_lossy(&line).into_owned());
                    line.clear();
                }
            }
        });
        Ok(Self {
            connection,
            process,
        })
    }

    pub fn pid(&self) -> u32 {
        self.process.pid
    }
    pub fn connection(&self) -> Connection {
        self.connection.clone()
    }
    pub async fn wait_exit(&self) -> Result<String> {
        let mut exited = self.process.exited.clone();
        loop {
            if let Some(result) = exited.borrow_and_update().clone() {
                return result;
            }
            exited
                .changed()
                .await
                .map_err(|_| error(4, "进程等待任务已结束"))?;
        }
    }
    pub async fn stop(&self, reason: &str) -> Result<()> {
        self.connection.terminate(reason);
        self.process.stop.send_replace(true);
        self.wait_exit().await.map(|_| ())
    }
    pub async fn shutdown(&self, duration: Duration) -> Result<()> {
        self.process.shutting_down.store(true, Ordering::Release);
        let response = self.request("shutdown", Value::Null, duration).await;
        if response.is_ok() {
            self.connection.terminate("插件已确认有序退出");
        }
        match timeout(duration, self.wait_exit()).await {
            Ok(result) => {
                result?;
            }
            Err(_) => self.stop("插件有序退出超时").await?,
        }
        response.map(|_| ())
    }
}

fn observe_exit(pid: u32) -> Result<bool> {
    let pid = rustix::process::Pid::from_raw(pid as i32).expect("child PID is positive");
    waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG,
    )
    .map(|state| state.is_some())
    .map_err(|problem| error(1, problem.to_string()))
}

async fn terminate_and_reap(child: &mut Child, pid: u32) -> Result<String> {
    let group = Pid::from_raw(pid as i32);
    let signal_error = match killpg(group, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => None,
        Err(Errno::EPERM) if observe_exit(pid)? => Some(Errno::EPERM),
        Err(problem) => return Err(error(1, format!("终止插件进程组失败：{problem}"))),
    };
    let status = child.wait().await.map_err(io_error)?;
    // ADR-0027：回收后只查询组是否已空，终止信号已在PID受保有期间发出。
    if let Some(problem) = signal_error
        && killpg(group, None) != Err(Errno::ESRCH)
    {
        return Err(error(1, format!("终止插件进程组失败：{problem}")));
    }
    Ok(format!("插件进程退出：{status}"))
}

fn error(code: u32, message: impl Into<String>) -> WireError {
    WireError::new(code, message)
}
fn io_error(problem: std::io::Error) -> WireError {
    error(1, problem.to_string())
}
