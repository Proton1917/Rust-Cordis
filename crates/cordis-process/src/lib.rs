#![cfg_attr(unix, forbid(unsafe_code))]
//! CP-7、CP-10、ADR-0027/0029：持有子进程、组信号和回收的唯一执行路径。
//! 插件连接与有界探测共享该路径；输出、退出状态和清理错误分别交给调用方。

pub mod cache;
mod platform;

use cordis_up::{
    Value, WireError,
    rpc::{Connection, ConnectionOptions, Handler},
};
use platform::ChildGuard;
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
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader},
    process::{Child, ChildStderr, ChildStdout},
    sync::watch,
    time::timeout,
};

type Result<T> = std::result::Result<T, WireError>;
type Log = Arc<dyn Fn(u64, String) + Send + Sync>;

#[derive(Clone, Debug)]
/// CP-2、CP-3：连接预算与诊断行上限，在启动边界确定。
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
/// CP-7：共享进程拥有者，最后句柄释放时提交终止请求。
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
        if options.stderr_line_bytes == 0 {
            return Err(error(2, "进程命令与日志行容量需要有效取值"));
        }
        let (mut child, mut guard) = platform::spawn(command, None, cwd, Stdio::piped())?;
        let pid = guard.pid();
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let connection = match Connection::attach(id, stdout, stdin, options.rpc, handler) {
            Ok(connection) => connection,
            Err(problem) => {
                guard.terminate_and_reap(&mut child).await?;
                return Err(problem);
            }
        };
        let (exited, exit_status) = watch::channel(None);
        let (stop, stopping) = watch::channel(false);
        let shutting_down = Arc::new(AtomicBool::new(false));
        let process = Arc::new(Process {
            pid,
            exited: exit_status,
            stop,
            shutting_down: shutting_down.clone(),
        });
        tokio::spawn(
            Lifecycle {
                id,
                child,
                guard,
                connection: connection.clone(),
                stopping,
                shutting_down,
                exited,
                log: log.clone(),
            }
            .run(),
        );
        tokio::spawn(read_diagnostics(id, stderr, options.stderr_line_bytes, log));
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
    /// ADR-0027：等待组清理和直接子进程回收，保留实际系统错误。
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
    /// CP-6、CP-7：断开协议并由生命周期任务完成终止。
    pub async fn stop(&self, reason: &str) -> Result<()> {
        self.connection.terminate(reason);
        self.process.stop.send_replace(true);
        self.wait_exit().await.map(|_| ())
    }
    /// CP-4、CP-10：成功响应后关闭输入；超时转为显式组终止。
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

/// CP-7：进程控制与协议I/O分离，Child只由该任务回收。
struct Lifecycle {
    id: u64,
    child: Child,
    guard: ChildGuard,
    connection: Connection,
    stopping: watch::Receiver<bool>,
    shutting_down: Arc<AtomicBool>,
    exited: watch::Sender<Option<Result<String>>>,
    log: Log,
}

impl Lifecycle {
    async fn run(mut self) {
        let result = self.wait_and_reap().await;
        let reason = match &result {
            Ok(reason) => reason.clone(),
            Err(problem) => {
                (self.log)(self.id, problem.to_string());
                problem.to_string()
            }
        };
        self.connection.terminate(&reason);
        self.exited.send_replace(Some(result));
    }

    /// CP-6、CP-10：显式停止与异常EOF触发组终止，有序退出等待子进程结束。
    async fn wait_and_reap(&mut self) -> Result<String> {
        loop {
            if self.guard.exited()? || *self.stopping.borrow() {
                break;
            }
            tokio::select! {
                _ = self.stopping.changed() => break,
                result = self.guard.wait_signal(&mut self.child) => {
                    result?;
                }
                _ = self.connection.closed(), if !self.shutting_down.load(Ordering::Acquire) => {
                    if !self.shutting_down.load(Ordering::Acquire) { break; }
                }
            }
        }
        self.guard
            .terminate_and_reap(&mut self.child)
            .await
            .map(|status| format!("插件进程退出：{status}"))
    }
}

/// CP-3：诊断保留有界前缀，超长行持续排空至换行，读取错误记入日志。
async fn read_diagnostics(id: u64, stderr: ChildStderr, limit: usize, log: Log) {
    let mut input = BufReader::new(stderr);
    let mut line = Vec::new();
    let mut truncated = false;
    loop {
        let available = match input.fill_buf().await {
            Ok(bytes) => bytes,
            Err(problem) => {
                log(id, format!("标准错误读取失败：{problem}"));
                break;
            }
        };
        if available.is_empty() {
            if !line.is_empty() {
                let mut message = String::from_utf8_lossy(&line).into_owned();
                if truncated {
                    message.push_str(" [诊断已截断]");
                }
                log(id, message);
            }
            break;
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        let complete = available[count - 1] == b'\n';
        let content = count - usize::from(complete);
        let retained = content.min(limit - line.len());
        line.extend_from_slice(&available[..retained]);
        truncated |= retained < content;
        input.consume(count);
        if complete {
            let mut message = String::from_utf8_lossy(&line).into_owned();
            if truncated {
                message.push_str(" [诊断已截断]");
            }
            log(id, message);
            line.clear();
            truncated = false;
        }
    }
}

/// 有界描述符探测复用进程组所有权；stdout超限或截止时间到达时清理并回收整个组。
pub async fn run_capture(
    command: &[String],
    cwd: &Path,
    duration: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    run_capture_with_env(command, None, cwd, duration, max_bytes).await
}

/// 与 `run_capture` 共用进程组所有权，并允许附加子进程环境变量。
pub async fn run_capture_with_env(
    command: &[String],
    env: Option<&[(String, String)]>,
    cwd: &Path,
    duration: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    let (mut child, mut guard) = platform::spawn(command, env, cwd, Stdio::null())?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let capture = capture_child(&mut child, &mut guard, &mut stdout, &mut stderr, max_bytes);
    let result = match timeout(duration, capture).await {
        Ok(result) => result,
        Err(_) => Err(error(6, "描述符探测超过截止时间")),
    };
    match result {
        Ok((status, stdout, stderr)) if status.success() => {
            drop(stderr);
            Ok(stdout)
        }
        Ok((status, _, stderr)) => Err(error(
            1,
            format!("探测进程 {status}：{}", String::from_utf8_lossy(&stderr)),
        )),
        Err(problem) => {
            if child.id().is_some() {
                guard.terminate_and_reap(&mut child).await?;
            }
            Err(problem)
        }
    }
}

/// CP-7：保留标准流至组清理结束，避免输出超额时提前关闭管道改变退出状态。
async fn capture_child(
    child: &mut Child,
    guard: &mut ChildGuard,
    stdout: &mut ChildStdout,
    stderr: &mut ChildStderr,
    max_bytes: usize,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    let lifetime = async {
        while !guard.exited()? {
            guard.wait_signal(child).await?;
        }
        guard.terminate_and_reap(child).await
    };
    tokio::try_join!(
        lifetime,
        capture_output(stdout, max_bytes, true),
        capture_output(stderr, 4096, false)
    )
}

/// CP-2：stdout采用硬上限；诊断流保持有界文本并持续排空。
async fn capture_output(
    mut input: impl AsyncRead + Unpin,
    limit: usize,
    reject_extra: bool,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = input.read(&mut buffer).await.map_err(io_error)?;
        if read == 0 {
            return Ok(output);
        }
        let kept = read.min(limit.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..kept]);
        if reject_extra && kept != read {
            return Err(error(8, "描述符输出超过容量上限"));
        }
    }
}

fn error(code: u32, message: impl Into<String>) -> WireError {
    WireError::new(code, message)
}
fn io_error(problem: std::io::Error) -> WireError {
    error(1, problem.to_string())
}
