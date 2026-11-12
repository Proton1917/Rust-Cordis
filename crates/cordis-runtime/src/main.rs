//! DOC-15 §3.1：核心命令入口，开发命令经同目录 cordis-dev 执行。
#![forbid(unsafe_code)]

use clap::Parser;
use cordis_runtime::cli::{self, Action, Cli, ServiceAction};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let result = match Cli::parse().command {
        None => cordis_runtime::run("cordis.toml".into()).await.map(|_| 0),
        Some(Action::Run(arguments)) => cordis_runtime::run(arguments.config).await.map(|_| 0),
        Some(Action::Status(arguments)) => cli::status(&arguments.config).map(|status| {
            println!("{status}");
            0
        }),
        Some(Action::Version) => {
            println!("cordis {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Some(Action::Service {
            action: ServiceAction::Install(options),
        }) => cli::service(options).map(|path| {
            println!("{}", path.display());
            0
        }),
        Some(Action::Developer(arguments)) => cli::forward(&arguments),
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(problem) => {
            eprintln!("cordis: {problem}");
            std::process::exit(1);
        }
    }
}
