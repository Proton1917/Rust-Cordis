//! DOC-02 A-2、DOC-15 Q0：核心把内核任务和时钟接入自身 Tokio 执行器。

use cordis::{Clock, Spawn, Task};
use std::time::{Duration, Instant};

pub struct Executor {
    started: Instant,
    handle: tokio::runtime::Handle,
}

impl Executor {
    /// DOC-02 A-2：在已启动的 Tokio 运行时中捕获执行句柄与单调时钟起点。
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            handle: tokio::runtime::Handle::current(),
        }
    }
}

impl Spawn for Executor {
    fn spawn(&self, task: Task) {
        self.handle.spawn(task);
    }
}

impl Clock for Executor {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }
    fn sleep(&self, duration: Duration) -> Task {
        Box::pin(tokio::time::sleep(duration))
    }
}
