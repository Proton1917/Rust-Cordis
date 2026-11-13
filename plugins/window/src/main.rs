//! DOC-15 §11.6、CP-10：窗口主线程与 UP/1 工作线程共享实际资源的编号。
#![forbid(unsafe_code)]

mod protocol;
mod ui;

use cordis_up::WireError;

type Result<T> = std::result::Result<T, WireError>;

fn error(problem: impl std::fmt::Display) -> WireError {
    WireError::new(2, problem.to_string())
}

fn main() {
    let result = ui::run(|bridge, signals| {
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(problem) => {
                    eprintln!("窗口执行器启动失败：{problem}");
                    bridge.stop();
                    return;
                }
            };
            if let Err(problem) = runtime.block_on(protocol::run(bridge.clone(), signals)) {
                eprintln!("窗口插件：{problem}");
            }
            bridge.stop();
            runtime.shutdown_timeout(std::time::Duration::from_secs(1));
        })
    });
    if let Err(problem) = result {
        eprintln!("窗口界面：{problem}");
        std::process::exit(1);
    }
}
