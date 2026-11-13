//! DOC-13 §3.4、DOC-15 Q0：原生插件宿主提供 ABI 执行、库寿命和协议对象转发。

#![deny(unsafe_op_in_unsafe_fn)]

pub mod abi;
mod forward;
pub mod host;
mod jobs;
pub mod lease;
pub mod library;
pub mod native;
pub mod objects;
mod verification;
pub mod wire;

pub use cordis_abi::{Error, Result};
pub use library::{
    LibraryInfo, LibraryOptions, LibraryStore, NativeComponent, NativeLibrary, OpenError,
};
pub use objects::{ObjectInfo, Objects};
