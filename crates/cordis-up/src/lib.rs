//! DOC-16、DOC-15 CP-2–CP-5/CP-8：UP/1 数据与有界双向连接。
//! 编解码约束消息形状和分配规模，RPC层持有载荷资源并隔离连接故障。
#![forbid(unsafe_code)]

mod codec;
mod message;
pub mod payload;
pub mod rpc;

pub use ciborium::value::Value;
pub use codec::{CodecError, Limits, decode, decode_value, encode, encode_value};
pub use message::{Message, WireError};
pub use payload::{OBJECT_TAG, ObjectRef, payload_refs};
