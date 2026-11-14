//! 接口 {{name}}@1：共享值操作、状态模式与变化通知。

use cordis_sdk::prelude::*;

#[cordis_sdk::interface(name = "{{name}}", version = "1.0")]
pub trait Counter: Send + Sync {
    fn value(&self) -> Result<u64>;
    fn set(&self, value: u64) -> Result<()>;
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub value: u64,
}

#[cordis_sdk::event(name = "{{name}}.changed", version = "1.0")]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Changed {
    pub value: u64,
}
