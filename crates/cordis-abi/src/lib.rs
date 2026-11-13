//! DOC-12：稳定的 C ABI、边界所有权和源侧释放。
#![deny(unsafe_op_in_unsafe_fn)]

mod buffer;
mod callback;
mod descriptor;
mod encoding;
mod environment;
mod error;
mod future;
mod generic;
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
pub use encoding::{CborEncoder, decode_in, decode_resource, encode_resource, from_value_in};
pub use environment::Environment;
pub use error::{Error, Result, boundary, boundary_unit, decode, encode, entry_boundary};
pub use future::{RemoteFuture, make_future};
pub use generic::{
    CborValue, GenericService, InvokeFuture, OPTION_SOME_TAG, RESOURCE_TAG, decode_arguments,
    decode_value, from_value, generic_object, make_invoke_future, option_some, option_take,
    to_value,
};
pub use host::{DerivedContext, HostClient, State};
pub use object::{Object, ScopeFactory, object_arc, object_data, object_environment, object_scope};
pub use raw::*;
