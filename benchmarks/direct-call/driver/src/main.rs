//! DOC-15 AU-9、RM-9：独立宿主中交替测量两个真实 SDK 构建的类型化调用。
#![forbid(unsafe_code)]

#[path = "../../latency.rs"]
mod latency;

use clap::Parser;
use cordis::{Clock, Context, Spawn, Task};
use cordis_supervisor::{
    PeerOptions,
    manager::{GroupSpec, InstanceSpec, Manager, ManagerOptions, RestartPolicy},
};
use cordis_up::Value;
use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

struct Executor(Instant);
impl Spawn for Executor {
    fn spawn(&self, task: Task) {
        tokio::spawn(task);
    }
}
impl Clock for Executor {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, duration: Duration) -> Task {
        Box::pin(tokio::time::sleep(duration))
    }
}

struct Artifacts {
    host: PathBuf,
    provider: PathBuf,
    work: PathBuf,
}

#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    host: PathBuf,
    #[arg(long)]
    provider: PathBuf,
    #[arg(long)]
    settings: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

fn check(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message.into()).into())
    }
}

fn group(
    name: &str,
    directory: &Path,
    artifacts: &Artifacts,
    libraries: &[&Path],
) -> Result<GroupSpec> {
    std::fs::create_dir(directory)?;
    let mut command = vec![
        artifacts.host.to_string_lossy().into_owned(),
        "--cache-dir".into(),
        directory.join("lib").to_string_lossy().into_owned(),
        "--request-timeout-ms".into(),
        "5000".into(),
        "--unload".into(),
        "when-unleased".into(),
    ];
    for library in libraries {
        command.extend(["--library".into(), library.to_string_lossy().into_owned()]);
    }
    Ok(GroupSpec {
        name: name.into(),
        command,
        directory: directory.into(),
        peer: PeerOptions::default(),
        restart: RestartPolicy {
            max: 0,
            backoff: Duration::from_millis(500),
        },
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let mut settings: latency::Settings =
        serde_json::from_reader(std::fs::File::open(arguments.settings)?)?;
    settings.baseline = settings.baseline.canonicalize()?;
    settings.current = settings.current.canonicalize()?;
    if let Some(parent) = arguments.output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&arguments.output)?;
    let artifacts = Artifacts {
        host: arguments.host.canonicalize()?,
        provider: arguments.provider.canonicalize()?,
        work: arguments.output.canonicalize()?,
    };
    latency::run(&artifacts, &settings).await
}
