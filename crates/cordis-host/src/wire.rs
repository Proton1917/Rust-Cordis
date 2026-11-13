//! DOC-15 V-1、V-4：CBOR 资源引用在原生对象与 UP/1 资源表之间转移。

use crate::{abi, native::NativeExecutor, objects::Objects};
use cordis_abi::{
    Buf, CborEncoder, Error, GenericService, InvokeFuture, Object, Result, generic_object,
    make_invoke_future,
};
use cordis_up::{
    Value, WireError, payload,
    rpc::{Outgoing, Payload, RemoteObject},
};
use std::{collections::HashMap, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct Bridge {
    pub objects: Arc<Objects>,
    pub process: u64,
    pub executor: NativeExecutor,
    pub timeout: Duration,
}

impl Bridge {
    /// DOC-15 V-1、V-4、ADR-0046：宿主执行器轮询 RPC，插件线程接收结果；丢弃句柄传播取消。
    pub fn invoke(&self, object: &RemoteObject, method: &str, args: &[u8]) -> Result<InvokeFuture> {
        let mut params = payload::Invoke {
            object: object.id(),
            method: method.into(),
            args: cordis_abi::decode_arguments(args)?,
            refs: Vec::new(),
        };
        let mut exports = self.objects.export_batch(abi::environment());
        for argument in &mut params.args {
            exports.rewrite(argument)?;
        }
        params.refs = exports.refs().to_vec();
        let outgoing =
            Outgoing::with_commit(cordis_abi::to_value(&params)?, move || exports.commit());
        let bridge = self.clone();
        let object = object.clone();
        let task = self.executor.start(
            async move {
                let response = object
                    .peer()
                    .request("invoke", outgoing, bridge.timeout)
                    .await
                    .map_err(from_wire)?;
                let result: payload::InvokeResult =
                    cordis_abi::from_value(response.value().clone())?;
                bridge.incoming(&response, result.value).await
            },
            true,
        );
        make_invoke_future(task, abi::environment())
    }

    /// DOC-16 资源协议：结果中的本地拥有资源转换为待提交的协议编号。
    pub fn outgoing(&self, mut value: Value) -> Result<Outgoing> {
        let mut exports = self.objects.export_batch(abi::environment());
        exports.rewrite(&mut value)?;
        let result = payload::InvokeResult {
            value,
            refs: exports.refs().to_vec(),
        };
        Ok(Outgoing::with_commit(
            cordis_abi::to_value(&result)?,
            move || exports.commit(),
        ))
    }

    /// DOC-16 资源协议：同一资源编号复用一次导入，ABI 缓冲区持有全部导入对象。
    pub async fn incoming(&self, payload: &Payload, mut value: Value) -> Result<Buf> {
        let mut encoded = CborEncoder::new(abi::environment());
        let mut replacements: HashMap<u64, Value> = HashMap::new();
        for (id, target) in resource_locations(&mut value)? {
            let replacement = if let Some(value) = replacements.get(&id) {
                value.clone()
            } else {
                let reference = payload.import(id).map_err(from_wire)?;
                let metadata = reference
                    .peer()
                    .request(
                        "object_info",
                        cordis_abi::to_value(&payload::ObjectInfoRequest { object: id })?,
                        self.timeout,
                    )
                    .await
                    .map_err(from_wire)?;
                let info: payload::ObjectInfo = cordis_abi::from_value(metadata.value().clone())?;
                let object = self.import(reference, info)?;
                let value = encoded.export(object)?;
                replacements.insert(id, value.clone());
                value
            };
            *target = replacement;
        }
        encoded.finish(value)
    }

    /// DOC-15 §8：核心确认同组来源时复用本地对象，其余来源构造协议代理。
    pub fn import(&self, object: RemoteObject, info: payload::ObjectInfo) -> Result<Object> {
        if let Some(local) = info.local
            && local.process == self.process
        {
            return self.objects.import(local.object);
        }
        generic_object(
            info.interface,
            info.methods,
            Arc::new(RemoteService {
                bridge: self.clone(),
                object,
            }),
            abi::environment(),
        )
    }
}

struct RemoteService {
    bridge: Bridge,
    object: RemoteObject,
}

impl GenericService for RemoteService {
    fn invoke(&self, method: &str, args: &[u8]) -> Result<Buf> {
        self.bridge
            .executor
            .wait(self.bridge.invoke(&self.object, method, args)?)
    }

    fn invoke_async(&self, method: &str, args: &[u8]) -> Result<InvokeFuture> {
        self.bridge.invoke(&self.object, method, args)
    }
}

fn resource_id(value: &Value) -> Result<u64> {
    value
        .as_integer()
        .and_then(|id| u64::try_from(id).ok())
        .filter(|id| *id != 0)
        .ok_or_else(|| Error::new(2, "资源引用需要正 u64 编号"))
}

fn resource_locations(value: &mut Value) -> Result<Vec<(u64, &mut Value)>> {
    let mut pending = vec![value];
    let mut locations = Vec::new();
    while let Some(value) = pending.pop() {
        match value {
            Value::Tag(cordis_up::OBJECT_TAG, reference) => {
                let id = resource_id(reference)?;
                locations.push((id, value));
            }
            Value::Tag(_, value) => pending.push(value),
            Value::Array(values) => pending.extend(values),
            Value::Map(fields) => {
                for (key, value) in fields {
                    pending.push(key);
                    pending.push(value);
                }
            }
            _ => {}
        }
    }
    Ok(locations)
}

pub fn from_wire(error: WireError) -> Error {
    let error_value = Error::new(error.code, error.message);
    if matches!(error.detail, Value::Null) {
        error_value
    } else {
        match cordis_abi::encode(&error.detail) {
            Ok(detail) => error_value.with_detail(detail),
            Err(error) => error,
        }
    }
}

pub fn to_wire(error: Error) -> WireError {
    let detail = if error.detail().is_empty() {
        Value::Null
    } else {
        match cordis_abi::decode_value(error.detail()) {
            Ok(detail) => detail,
            Err(problem) => {
                return WireError::new(2, format!("错误详情编码无效：{problem}；原错误：{error}"));
            }
        }
    };
    WireError {
        code: error.code(),
        message: error.message().into(),
        detail,
    }
}
