//! DOC-15 Q0、§3.1：独立核心的配置、管理接口、状态持久化与进程部署。
#![forbid(unsafe_code)]

mod builtins;
mod catalog;
pub mod cli;
pub mod config;
pub mod contract;
mod control;
mod deployment;
mod executor;
mod install;
pub mod manifest;
mod request;
mod runtime;
mod status;
pub mod storage;

pub use runtime::{run, run_with_contract};
