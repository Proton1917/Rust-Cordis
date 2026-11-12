//! DOC-16 资源协议：本地拥有引用分配编号，待发送资源随响应提交或回收。

use crate::native::{NativeExecutor, Owned};
use cordis_abi::{AbiUnit, Buf, Environment, Error, InvokeFuture, Object, RemoteFuture, Result};
use cordis_up::Value;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub object: u64,
    pub interface: String,
    pub methods: Vec<String>,
    pub kind: u32,
}

/// DOC-13 §3.4：编号只记录本地拥有引用，绑定与生命周期由核心决定。
pub struct Objects {
    next: AtomicU64,
    values: Mutex<HashMap<u64, Arc<Owned<Object>>>>,
    executor: NativeExecutor,
}

impl Objects {
    pub fn new(executor: NativeExecutor) -> Self {
        Self {
            next: AtomicU64::new(1),
            values: Mutex::new(HashMap::new()),
            executor,
        }
    }

    pub fn insert(&self, object: Object) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.values
            .lock()
            .insert(id, Arc::new(self.executor.own(object)));
        id
    }

    pub fn borrow(&self, id: u64) -> Result<Arc<Owned<Object>>> {
        self.values
            .lock()
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::new(4, "对象编号已释放"))
    }

    pub fn import(&self, id: u64) -> Result<Object> {
        self.borrow(id)?.try_clone()
    }

    /// 为一次UP/1发送建立独立拥有引用，由对端release通知释放。
    pub fn export(&self, id: u64) -> Result<u64> {
        Ok(self.insert(self.import(id)?))
    }

    pub fn export_batch(self: &Arc<Self>, environment: Environment) -> PendingExports {
        PendingExports {
            objects: self.clone(),
            environment,
            mapping: HashMap::new(),
            refs: Vec::new(),
        }
    }

    pub fn release(&self, id: u64) -> Result<()> {
        let object = self
            .values
            .lock()
            .remove(&id)
            .ok_or_else(|| Error::new(4, "对象编号已释放"))?;
        drop(object);
        Ok(())
    }

    pub fn info(&self, id: u64) -> Result<ObjectInfo> {
        let object = self.borrow(id)?;
        Ok(ObjectInfo {
            object: id,
            interface: object.interface()?.to_owned(),
            methods: object.method_names()?,
            kind: object.header().kind,
        })
    }

    pub fn invoke(&self, id: u64, method: &str, args: &[u8]) -> Result<Buf> {
        self.borrow(id)?.invoke(method, args)
    }

    pub fn invoke_async(&self, id: u64, method: &str, args: &[u8]) -> Result<InvokeFuture> {
        self.borrow(id)?.invoke_async(method, args)
    }

    pub fn callback(&self, id: u64, context: u64, scope: u64, payload: &[u8]) -> Result<()> {
        self.borrow(id)?.call_sync(context, scope, payload)
    }

    pub fn callback_async(
        &self,
        id: u64,
        context: u64,
        scope: u64,
        payload: &[u8],
    ) -> Result<RemoteFuture<AbiUnit>> {
        self.borrow(id)?.call_async(context, scope, payload)
    }

    pub fn export_state(&self, id: u64) -> Result<Buf> {
        self.borrow(id)?.export()
    }
    pub fn thaw(&self, id: u64) -> Result<()> {
        self.borrow(id)?.thaw()
    }

    pub fn clear(&self) {
        let values = std::mem::take(&mut *self.values.lock());
        drop(values);
    }

    pub fn len(&self) -> usize {
        self.values.lock().len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.lock().is_empty()
    }
}

/// 出口引用在发送队列接管前由本地拥有，commit对应Outgoing的入队交接点。
pub struct PendingExports {
    objects: Arc<Objects>,
    environment: Environment,
    mapping: HashMap<u64, u64>,
    refs: Vec<u64>,
}

impl PendingExports {
    pub fn insert(&mut self, object: Object) -> u64 {
        let id = self.objects.insert(object);
        self.refs.push(id);
        id
    }

    pub fn export(&mut self, id: u64) -> Result<u64> {
        if let Some(exported) = self.mapping.get(&id) {
            return Ok(*exported);
        }
        let exported = self.objects.export(id)?;
        self.refs.push(exported);
        self.mapping.insert(id, exported);
        Ok(exported)
    }

    pub fn rewrite(&mut self, value: &mut Value) -> Result<()> {
        let mut pending = vec![value];
        while let Some(value) = pending.pop() {
            match value {
                Value::Tag(cordis_up::OBJECT_TAG, payload) => {
                    let id = payload
                        .as_integer()
                        .and_then(|id| u64::try_from(id).ok())
                        .filter(|id| *id != 0)
                        .ok_or_else(|| Error::new(2, "服务引用需要正 u64 编号"))?;
                    **payload = Value::Integer(self.export(id)?.into());
                }
                Value::Tag(_, payload) => pending.push(payload),
                Value::Array(values) => pending.extend(values.iter_mut()),
                Value::Map(fields) => {
                    for (key, value) in fields {
                        pending.push(key);
                        pending.push(value);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn refs(&self) -> &[u64] {
        &self.refs
    }

    pub fn commit(mut self) {
        self.refs.clear();
    }
}

impl Drop for PendingExports {
    fn drop(&mut self) {
        for id in self.refs.drain(..) {
            if let Err(error) = self.objects.release(id) {
                self.environment.report(error);
            }
        }
    }
}
