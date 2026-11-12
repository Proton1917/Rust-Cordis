//! DOC-12：稳定的 C ABI、边界所有权和源侧释放。
#![deny(unsafe_op_in_unsafe_fn)]

mod buffer;
mod callback;
mod descriptor;
mod environment;
mod error;
mod future;
mod host;
mod object;
mod raw;

#[cfg(feature = "test-support")]
pub mod testing;

pub use buffer::Buf;
pub use callback::{async_callback, state_exporter, sync_callback};
pub use descriptor::{
    ComponentDefinition, Dependency, Entry, PluginDescriptor, PluginEntry, PluginMetadata,
    component_at,
};
pub use environment::Environment;
pub use error::{Error, Result, boundary, boundary_unit, decode, encode, entry_boundary};
pub use future::{RemoteFuture, make_future};
pub use host::{HostClient, State};
pub use object::{Object, object_arc, object_data, object_environment};
pub use raw::*;
