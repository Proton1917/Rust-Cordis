extern crate self as cordis_sdk;

pub mod builtin;
mod bytes;
mod context;
mod metadata;
mod model;
mod service;

pub use bytes::Bytes;
pub use context::{Context, Effect, EffectScope};
pub use cordis_abi::{CborValue, Error, Result, decode, encode};
pub use cordis_types::{Interception, Isolation};
pub use metadata::{Caller, caller};
pub type ContextOptions = cordis_types::ContextOptions<CborValue>;
pub use cordis_sdk_macros::{event, export_plugin, interface};
pub use model::{
    AsyncEvent, BoxFuture, Component, Dependency, EmptyConfig, Event, Frozen, Inject, Listens,
    Provides, StateBlob, StateSchema, SyncEvent,
};
pub use service::{BorrowedService, Interface, Scoped, Service};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_INFO: &str = concat!(env!("CORDIS_RUSTC"), " ", env!("CORDIS_TARGET"));

pub mod prelude {
    pub use crate::{
        AsyncEvent, BorrowedService, BoxFuture, Bytes, CborValue, Component, Context,
        ContextOptions, Effect, EffectScope, EmptyConfig, Error, Event, Frozen, Inject,
        Interception, Interface, Isolation, Listens, Provides, Result, Scoped, Service, StateBlob,
        StateSchema, SyncEvent,
    };
    pub use std::sync::Arc;
}

#[doc(hidden)]
pub mod __private {
    pub use cordis_abi as abi;

    pub fn decode_bool(value: u8) -> crate::Result<bool> {
        match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(crate::Error::new(2, "ABI 布尔值需要为 0 或 1")),
        }
    }

    pub fn check_interface(object: &abi::Object, expected: &str) -> crate::Result<()> {
        let actual = object.interface()?;
        if object.header().kind == 1
            && actual.rsplit_once('.').map(|(key, _)| key)
                == expected.rsplit_once('.').map(|(key, _)| key)
        {
            Ok(())
        } else {
            Err(crate::Error::new(
                2,
                format!("接口对象 {actual} 与 {expected} 的名称或主版本不同"),
            ))
        }
    }

    pub fn validate_config<C: crate::Component>(bytes: &[u8]) -> crate::Result<C::Config> {
        let result = crate::decode::<C::Config>(bytes).and_then(|config| {
            C::resolve(&config)?;
            Ok(config)
        });
        result.map_err(|error| {
            if !error.detail().is_empty() {
                return error;
            }
            #[derive(serde::Serialize)]
            struct Issue<'a> {
                path: &'a str,
                message: &'a str,
            }
            let issue = [Issue {
                path: "",
                message: error.message(),
            }];
            match crate::encode(&issue) {
                Ok(detail) => crate::Error::new(2, error.message()).with_detail(detail),
                Err(encoding) => encoding,
            }
        })
    }
}
