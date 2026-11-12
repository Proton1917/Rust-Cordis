//! DOC-15 Q0、CP-1：独立宿主进程通过标准输入输出执行核心命令。

use clap::{Parser, ValueEnum};
use cordis_host::{
    abi,
    host::{Host, HostOptions},
    lease::UnloadPolicy,
    library::LibraryOptions,
};
use cordis_up::{
    Limits,
    rpc::{Connection, ConnectionOptions},
};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone, Copy, ValueEnum)]
enum Unload {
    Never,
    WhenUnleased,
}

#[derive(Parser)]
#[command(name = "cordis-host", version)]
struct Arguments {
    #[arg(long, value_name = "PATH", conflicts_with = "libraries")]
    describe: Option<PathBuf>,
    #[arg(long = "library", value_name = "PATH")]
    libraries: Vec<PathBuf>,
    #[arg(long, default_value = ".cordis/lib")]
    cache_dir: PathBuf,
    #[arg(long, value_enum, default_value = "never")]
    unload: Unload,
    #[arg(long)]
    allow_panic_abort: bool,
    #[arg(long)]
    require_verified: bool,
    #[arg(long, default_value_t = 2)]
    native_workers: usize,
    #[arg(long, default_value_t = 30_000)]
    request_timeout_ms: u64,
    #[arg(long, default_value_t = 64)]
    max_inflight: usize,
    #[arg(long, default_value_t = 64)]
    queue_capacity: usize,
    #[arg(long, default_value_t = 3)]
    consecutive_timeouts: u32,
    #[arg(long, default_value_t = 16 * 1024 * 1024)]
    max_frame: usize,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let code = match run(Arguments::parse()).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    };
    std::process::exit(code);
}

async fn run(args: Arguments) -> cordis_host::Result<i32> {
    if args.require_verified {
        return Err(cordis_host::Error::new(3, "当前宿主缺少验证印记校验能力"));
    }
    let timeout = Duration::from_millis(args.request_timeout_ms);
    let host = Host::new(
        HostOptions {
            libraries: args.libraries,
            library: LibraryOptions {
                directory: args.cache_dir,
                unload: match args.unload {
                    Unload::Never => UnloadPolicy::Never,
                    Unload::WhenUnleased => UnloadPolicy::WhenUnleased,
                },
                allow_panic_abort: args.allow_panic_abort,
            },
            request_timeout: timeout,
            native_workers: args.native_workers,
        },
        tokio::runtime::Handle::current(),
    )?;
    abi::install(host.clone())?;
    if let Some(path) = args.describe {
        let description = host.describe_file(path).await?;
        serde_json::to_writer(std::io::stdout(), &description)
            .map_err(|error| cordis_host::Error::new(1, error.to_string()))?;
        println!();
        return Ok(0);
    }
    let options = ConnectionOptions {
        limits: Limits {
            max_frame: args.max_frame,
            ..Limits::default()
        },
        max_inflight: args.max_inflight,
        queue_capacity: args.queue_capacity,
        request_timeout: timeout,
        consecutive_timeouts: args.consecutive_timeouts,
    };
    let connection = Connection::attach(
        0,
        tokio::io::stdin(),
        tokio::io::stdout(),
        options,
        Arc::new(host.clone()),
    )
    .map_err(cordis_host::wire::from_wire)?;
    let reason = connection.closed().await;
    if host.stopped() {
        Ok(0)
    } else {
        eprintln!("UP/1 连接结束：{reason}");
        Ok(1)
    }
}
