//! UP/1 的消息、CBOR 编解码与资源编号。
#![forbid(unsafe_code)]

mod codec;
mod message;
pub mod payload;
pub mod rpc;

pub use ciborium::value::Value;
pub use codec::{CodecError, Limits, decode, encode};
pub use message::{Message, WireError};
pub use payload::{OBJECT_TAG, ObjectRef, payload_refs};
