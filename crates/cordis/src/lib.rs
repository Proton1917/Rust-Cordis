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
pub use events::{
    AsyncEvent, AsyncKind, AsyncListener, Dispatch, DispatchMode, Event, EventOptions,
    InternalDispatch, InternalListener, ListenerRegistration, Next, SyncEvent, SyncKind,
    SyncListener, WaterfallEvent, WaterfallKind, WaterfallListener,
};
pub use fiber::{
    Epoch, Fiber, FiberState, InternalCommit, InternalConfig, InternalPlugin, InternalStatus,
    InternalUpdate,
};
pub use kernel::{Clock, Spawn, Task};
pub use registry::{
    Component, ComponentDef, ComponentId, Config, Inject, RawConfig, Registry, ValidationError,
    ValidationIssue,
};
pub use service::{Merge, ServiceKey};
