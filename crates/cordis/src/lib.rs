//! 基于可回退效应与反应式余效应的组件运行时。

mod context;
mod effect;
mod error;
mod events;
mod fiber;
mod kernel;
mod list;
mod logger;
mod reflect;
mod registry;
mod service;

pub use context::{Context, Realm};
pub use effect::{Effect, EffectMeta, EffectScope};
pub use error::{Error, Result};
pub use fiber::{Fiber, FiberState};
pub use kernel::{Clock, Spawn, Task};
pub use service::{Merge, ServiceKey};
