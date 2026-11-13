//! ADR-0039、DOC-12 §5/§7：同步 Serde 作用域传递资源环境，缓冲拥有导出引用。

use crate::{
    Buf, CborValue, Environment, Error, Object, RESOURCE_TAG, Result, decode_value, encode,
};
use serde::{Serialize, de::DeserializeOwned};
use std::sync::{Arc, Mutex};

enum Scope {
    Plain,
    Encoding(Arc<Exports>),
    Decoding(Environment),
}

scoped_tls::scoped_thread_local!(static CURRENT: Scope);

struct Exports {
    environment: Environment,
    references: Mutex<Vec<u64>>,
}

impl Exports {
    fn export(&self, object: Object) -> Result<CborValue> {
        let id = self.environment.host().object_export(object)?;
        self.references.lock().expect("资源编号集合锁").push(id);
        Ok(CborValue::Tag(
            RESOURCE_TAG,
            Box::new(CborValue::Integer(id.into())),
        ))
    }

    fn release(&self, references: impl IntoIterator<Item = u64>) {
        for id in references {
            if let Err(error) = self.environment.host().object_release(id) {
                self.environment.report(error);
            }
        }
    }

    fn rollback(&self, start: usize) {
        let references = self
            .references
            .lock()
            .expect("资源编号集合锁")
            .split_off(start);
        self.release(references);
    }
}

impl Drop for Exports {
    fn drop(&mut self) {
        let references = std::mem::take(self.references.get_mut().expect("资源编号集合锁"));
        self.release(references);
    }
}

struct Transaction {
    exports: Arc<Exports>,
    start: usize,
    committed: bool,
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !self.committed {
            self.exports.rollback(self.start);
        }
    }
}

/// DOC-12 §7、ADR-0039：每个编码器拥有独立的导出列表，可随返回缓冲移交线程。
pub struct CborEncoder {
    exports: Arc<Exports>,
}

impl CborEncoder {
    pub fn new(environment: Environment) -> Self {
        Self {
            exports: Arc::new(Exports {
                environment,
                references: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn export(&mut self, object: Object) -> Result<CborValue> {
        self.exports.export(object)
    }

    /// ADR-0039：聚合值沿 Serde 递归导出服务，错误或展开回收本轮新增引用。
    pub fn value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<CborValue> {
        let start = self
            .exports
            .references
            .lock()
            .expect("资源编号集合锁")
            .len();
        let mut transaction = Transaction {
            exports: self.exports.clone(),
            start,
            committed: false,
        };
        let scope = Scope::Encoding(self.exports.clone());
        let result = CURRENT.set(&scope, || {
            CborValue::serialized(value).map_err(|problem| Error::new(2, problem.to_string()))
        });
        transaction.committed = result.is_ok();
        result
    }

    pub fn encode<T: Serialize + ?Sized>(mut self, value: &T) -> Result<Buf> {
        let value = self.value(value)?;
        self.finish(value)
    }

    pub fn finish(self, value: CborValue) -> Result<Buf> {
        let environment = self.exports.environment;
        let bytes = encode(&value)?;
        Buf::with_owner(bytes, self, environment)
    }
}

/// ADR-0039：普通状态编解码具有明确的纯数据作用域，嵌套调用完成后恢复外层。
pub(crate) fn plain<R>(operation: impl FnOnce() -> R) -> R {
    if CURRENT.is_set() {
        CURRENT.set(&Scope::Plain, operation)
    } else {
        operation()
    }
}

/// ADR-0039：Service 的 Serde 实现从当前编码事务取得独立对象引用。
#[doc(hidden)]
pub fn encode_resource(object: &Object) -> Result<CborValue> {
    let exports = if CURRENT.is_set() {
        CURRENT.with(|scope| match scope {
            Scope::Encoding(exports) => Some(exports.clone()),
            _ => None,
        })
    } else {
        None
    }
    .ok_or_else(|| Error::new(2, "服务值需要调用期编码环境"))?;
    exports.export(object.try_clone()?)
}

/// ADR-0039：导入值获得独立引用；聚合反序列化的失败由各已导入字段的 Drop 回收。
#[doc(hidden)]
pub fn decode_resource(value: &CborValue) -> Result<Object> {
    let environment = if CURRENT.is_set() {
        CURRENT.with(|scope| match scope {
            Scope::Decoding(environment) => Some(*environment),
            _ => None,
        })
    } else {
        None
    }
    .ok_or_else(|| Error::new(2, "服务值需要调用期解码环境"))?;
    environment.host().import_resource(value)
}

pub fn from_value_in<T: DeserializeOwned>(value: CborValue, environment: Environment) -> Result<T> {
    CURRENT.set(&Scope::Decoding(environment), || {
        value
            .deserialized()
            .map_err(|problem| Error::new(2, problem.to_string()))
    })
}

pub fn decode_in<T: DeserializeOwned>(bytes: &[u8], environment: Environment) -> Result<T> {
    from_value_in(decode_value(bytes)?, environment)
}
