//! Rust-cordis 原生插件 SDK。
//!
//! # 执行模型
//!
//! ADR-0051：同一原生分组的服务调用及 `Context::spawn` 任务共用宿主执行器，
//! 缺省为两个工作线程。异步方法与后台任务通过 `.await` 交还轮询权；同步方法
//! 需要异步结果时使用 [`Context::wait`]。自行 `futures::executor::block_on`
//! 会持续占用工作线程，多个并发等待可能耗尽同组的执行能力。
//!
//! `Context::wait` 由 cordis-host 0.1.1（ABI 1.1）交出工作线程，支持借用 Future、
//! 嵌套等待及 `Context::spawn_thread` 的系统线程。旧宿主在轮询前返回 code 3。
//! 跨进程调用继续遵守截止时间；可能超过截止时间的业务采用提交与查询接口。
//!
//! # tokio
//!
//! ADR-0055：默认开启的 `tokio` feature 为每个插件库提供一个 tokio 运行时上下文。
//! 插件代码可以直接使用 `tokio::spawn`、`tokio::time` 以及依赖 tokio 的库（如 reqwest）。
//! 全局 `tokio::spawn` 的任务属于插件库而非实例；需要随实例停用而结束的后台工作
//! 仍使用 [`Context::spawn`]。不需要 tokio 的插件可以用 `default-features = false` 关闭。

extern crate self as cordis_sdk;

mod ambient;
pub mod builtin;
mod bytes;
mod context;
mod dynamic;
mod metadata;
mod model;
mod service;

pub use bytes::Bytes;
pub use context::{Context, Effect, EffectScope};
pub use cordis_abi::{CborValue, Error, Result, decode, encode};
pub use cordis_types::{Interception, Isolation};
pub use dynamic::DynamicService;
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
        ContextOptions, DynamicService, Effect, EffectScope, EmptyConfig, Error, Event, Frozen,
        Inject, Interception, Interface, Isolation, Listens, Provides, Result, Scoped, Service,
        StateBlob, StateSchema, SyncEvent,
    };
    pub use std::sync::Arc;
}

#[doc(hidden)]
pub mod __private {
    pub use crate::ambient::{install as install_ambient, running as ambient_running, set_unload};
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
