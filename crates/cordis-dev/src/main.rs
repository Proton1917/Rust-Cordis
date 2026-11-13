#![forbid(unsafe_code)]
//! DOC-15 §3.1、§12.1：独立开发命令生成项目与插件，运行管理交由 cordis。

#[path = "../../../tools/build_paths.rs"]
mod build_paths;
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
    /// 在现有项目中创建插件并登记实例。
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
    /// 生成 TypeScript 进程插件。
    #[arg(long = "ts", group = "language")]
    typescript: bool,
    /// 生成网页部分，可与后端语言组合。
    #[arg(long)]
    web: bool,
    /// 生成普通 HTTP 路由处理方。
    #[arg(long, conflicts_with = "web")]
    http: bool,
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
        Action::NewPlugin(options) => create_plugin(options)?,
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

/// DOC-15 §12.1：后端语言与网页部分分别选择，按生成内容给出构建入口。
fn create_plugin(options: NewPlugin) -> Result<()> {
    if options.http {
        let language = if options.python {
            scaffold::Backend::Python
        } else if options.typescript {
            scaffold::Backend::TypeScript
        } else {
            scaffold::Backend::Rust
        };
        scaffold::http_plugin(&options.project, &options.name, language)?;
        println!("HTTP 路由与构建方法：plugins/{}/README.md", options.name);
    } else if options.web {
        let backend = if options.python {
            Some(scaffold::Backend::Python)
        } else if options.typescript {
            Some(scaffold::Backend::TypeScript)
        } else if options.rust {
            Some(scaffold::Backend::Rust)
        } else {
            None
        };
        scaffold::web_plugin(&options.project, &options.name, backend)?;
        println!("网页入口与构建方法：plugins/{}/README.md", options.name);
    } else if options.python {
        scaffold::python_plugin(&options.project, &options.name)?;
        println!(
            "安装插件依赖：python -m pip install --target plugins/{0}/.dependencies -r plugins/{0}/requirements.txt",
            options.name
        );
    } else if options.typescript {
        scaffold::typescript_plugin(&options.project, &options.name)?;
        println!(
            "安装插件依赖：npm --prefix plugins/{} install",
            options.name
        );
        println!("构建插件：npm --prefix plugins/{} run build", options.name);
    } else {
        scaffold::plugin(&options.project, &options.name)?;
    }
    println!("已创建插件 plugins/{}", options.name);
    Ok(())
}
