//! ADR-0017、ADR-0041：核心内置服务的调用身份、参数持有和发送后动作。

use crate::{Result, error};
use cordis_up::{
    Value,
    rpc::{HandlerFuture, RemoteObject},
};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct CallContext {
    pub connection: u64,
    pub context: u64,
    pub instance: String,
    pub group: String,
    pub plugin: String,
}

pub struct Arguments {
    pub values: Vec<Value>,
    pub(crate) objects: HashMap<u64, RemoteObject>,
}

impl Arguments {
    /// DOC-16 §6：调用期间持有参数资源，导入为独立本地引用。
    pub fn import(&self, id: u64) -> Result<RemoteObject> {
        self.objects
            .get(&id)
            .cloned()
            .ok_or_else(|| error(4, "参数资源编号缺席"))
    }
}

pub trait LocalService: Send + Sync + 'static {
    fn invoke<'a>(
        &'a self,
        context: &'a CallContext,
        method: &'a str,
        args: &'a Arguments,
    ) -> HandlerFuture<'a, Result<LocalReply>>;
}

pub struct LocalReply {
    pub value: Value,
    pub after_write: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl From<Value> for LocalReply {
    fn from(value: Value) -> Self {
        Self {
            value,
            after_write: None,
        }
    }
}

impl LocalReply {
    pub fn new(value: Value) -> Self {
        value.into()
    }

    /// DOC-16 §6：在回复写出后执行生命周期动作，写入失败按 Outgoing 释放。
    pub fn with_after_write(value: Value, action: impl FnOnce() + Send + 'static) -> Self {
        Self {
            value,
            after_write: Some(Box::new(action)),
        }
    }
}
