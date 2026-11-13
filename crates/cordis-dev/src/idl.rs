//! DOC-15 §7、ADR-0039：公开命令读取 WIT 并保存指定语言的绑定源码。

use crate::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Action {
    /// 根据 WIT 生成提供方接口、数据类型与调用方代理。
    Gen(Options),
}

#[derive(Args)]
pub struct Options {
    input: PathBuf,
    #[arg(long, value_parser = ["rust", "python", "ts"])]
    lang: String,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    feature: Vec<String>,
}

/// DOC-15 §7：解析与生成成功后写入目标；输入、类型或写入错误交回命令调用者。
pub fn run(action: Action) -> Result<()> {
    let Action::Gen(options) = action;
    let document = cordis_idl::Document::load(&options.input, &options.feature)?;
    let source = cordis_idl::generate(&document, &options.lang)?;
    std::fs::write(&options.output, source)?;
    println!("已生成 {}", options.output.display());
    Ok(())
}
