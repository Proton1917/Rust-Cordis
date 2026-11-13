//! ADR-0041、DOC-16 §6：核心对象出口目录与拥有型服务转发。
#![forbid(unsafe_code)]

mod directory;
mod interface;
mod local;
pub mod metadata;
mod transfer;

pub use directory::{Batch, Router};
pub use interface::Interface;
pub use local::{Arguments, CallContext, LocalReply, LocalService};

use cordis_up::{Value, WireError};
use serde::{Serialize, de::DeserializeOwned};

pub type Result<T = ()> = std::result::Result<T, WireError>;

fn error(code: u32, message: impl Into<String>) -> WireError {
    WireError::new(code, message)
}

/// DOC-16 §4：协议 DTO 通过共享值类型编码，字段错误保留 code 2。
fn value<T: Serialize + ?Sized>(data: &T) -> Result<Value> {
    Value::serialized(data).map_err(|problem| error(2, problem.to_string()))
}

fn decode<T: DeserializeOwned>(data: &Value) -> Result<T> {
    data.deserialized()
        .map_err(|problem| error(2, problem.to_string()))
}
