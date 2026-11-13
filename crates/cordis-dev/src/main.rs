#![forbid(unsafe_code)]
//! DOC-15 §3.1、§12.1：独立开发命令生成项目与插件，运行管理交由 cordis。

mod idl;
mod pack;
mod receipts;
mod scaffold;
mod tooling;
mod verify;

use clap::{Args, Parser, Subcommand};
use std::{path::PathBuf, process::ExitCode};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(name = "cordis-dev", version, about = "Cordis 插件开发工具")]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// 创建带 watch 与示例插件的 Rust 项目。
    New(NewProject),
    /// 在现有项目中创建 Rust 插件并登记实例。
    NewPlugin(NewPlugin),
    /// 通过独立核心运行契约套件，保存已验证库副本。
    Verify(verify::Options),
    /// 构建并装配包含核心、宿主与插件的发布包。
    Pack(pack::Options),
    /// 从 WIT 定义生成各语言接口、数据类型与调用方代理。
    #[command(subcommand)]
    Idl(idl::Action),
    /// 显示开发工具版本与支持的核心版本范围。
    Version,
}

#[derive(Args)]
struct NewProject {
    path: PathBuf,
    /// 使用本机 SDK 源码。
    #[arg(long)]
    sdk_path: Option<PathBuf>,
}

#[derive(Args)]
struct NewPlugin {
    name: String,
    #[arg(long, default_value = ".")]
    project: PathBuf,
    /// 生成 Rust 原生插件。
    #[arg(long, group = "language")]
    rust: bool,
    /// 生成 Python 进程插件。
    #[arg(long, group = "language")]
    python: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("{problem}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Action::New(options) => {
            scaffold::project(&options.path, options.sdk_path.as_deref())?;
            println!("已创建项目 {}", options.path.display());
            println!("进入项目后运行 cargo build --target-dir target，再运行 cordis run。");
        }
        Action::NewPlugin(options) => {
            if options.python {
                scaffold::python_plugin(&options.project, &options.name)?;
                println!(
                    "安装插件依赖：python -m pip install --target plugins/{0}/.dependencies -r plugins/{0}/requirements.txt",
                    options.name
                );
            } else {
                scaffold::plugin(&options.project, &options.name)?;
            }
            println!("已创建插件 plugins/{}", options.name);
        }
        Action::Verify(options) => verify::run(options).await?,
        Action::Pack(options) => pack::run(options).await?,
        Action::Idl(action) => idl::run(action)?,
        Action::Version => println!(
            "cordis-dev {}\n支持核心 >=0.1.0,<0.2.0",
            env!("CARGO_PKG_VERSION")
        ),
    }
    Ok(())
}
