//! CP-3、ADR-0041：连接保留的接口描述、对象信息和诊断内容使用共同字节额度。

use crate::{Interface, Result, decode, error, value};
use cordis_up::{
    Value, WireError, payload,
    rpc::{Connection, RemoteObject},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

pub const LOG_BYTES: usize = 4096;

/// CP-3、ADR-0041：协议诊断在 UTF-8 字符边界截断，保留容量包含结束标记。
pub fn clipped(text: &str) -> String {
    if text.len() <= LOG_BYTES {
        return text.to_owned();
    }
    const END: &str = "[已截断]";
    let mut end = LOG_BYTES - END.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = text[..end].to_owned();
    result.push_str(END);
    result
}

/// CP-3：连接退役后的诊断保留错误码和有界说明，资源载荷随连接释放。
pub fn saved_error(problem: &WireError) -> WireError {
    WireError {
        code: problem.code,
        message: clipped(&problem.message),
        detail: Value::Null,
    }
}

pub struct MetadataLease {
    counter: Arc<AtomicUsize>,
    bytes: usize,
}
impl Drop for MetadataLease {
    fn drop(&mut self) {
        self.counter.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

pub struct RetainedInfo {
    pub value: payload::ObjectInfo,
    pub _budget: Option<Arc<MetadataLease>>,
}
pub struct RetainedError {
    pub value: WireError,
    pub _budget: Option<Arc<MetadataLease>>,
}

pub struct MetadataBudget {
    counters: Mutex<HashMap<u64, Arc<AtomicUsize>>>,
    limit: usize,
}
impl MetadataBudget {
    pub fn new(limit: usize) -> Self {
        Self {
            counters: Mutex::new(HashMap::new()),
            limit,
        }
    }

    /// CP-3：边界登记取得额度，已有租约与连接目录分别拥有计数器。
    pub fn retain(&self, connection: &Connection, bytes: usize) -> Result<Arc<MetadataLease>> {
        let counter = {
            let mut counters = self.counters.lock();
            if !connection.is_alive() {
                return Err(error(4, "元数据所属连接已结束"));
            }
            counters
                .entry(connection.id())
                .or_insert_with(|| Arc::new(AtomicUsize::new(0)))
                .clone()
        };
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= self.limit)
            })
            .map_err(|_| error(8, "该连接的保留元数据内容已达上限"))?;
        Ok(Arc::new(MetadataLease { counter, bytes }))
    }

    /// DOC-16 §6：对象接口与方法名称共同持有保留额度。
    pub fn retain_info(
        &self,
        connection: &Connection,
        info: payload::ObjectInfo,
    ) -> Result<RetainedInfo> {
        let budget = self.retain(
            connection,
            strings(
                std::iter::once(info.interface.as_str())
                    .chain(info.methods.iter().map(String::as_str)),
            )?,
        )?;
        Ok(RetainedInfo {
            value: info,
            _budget: Some(budget),
        })
    }

    /// CP-6：连接目录退役，已在调用中持有的租约按各自引用周期释放。
    pub fn reclaim(&self, connection: u64) {
        self.counters.lock().remove(&connection);
    }

    /// DOC-15 CP-3、CP-6：保留真实错误；额度耗尽时结束连接并交回有界诊断。
    pub fn retain_error(
        &self,
        connection: &Connection,
        problem: &WireError,
        exhausted: impl FnOnce(&WireError),
    ) -> Result<RetainedError> {
        if !connection.is_alive() {
            return Ok(RetainedError {
                value: saved_error(problem),
                _budget: None,
            });
        }
        let bytes = problem
            .message
            .len()
            .checked_add(value_bytes(&problem.detail)?)
            .ok_or_else(|| error(8, "诊断长度超出地址空间"))?;
        match self.retain(connection, bytes) {
            Ok(budget) => Ok(RetainedError {
                value: problem.clone(),
                _budget: Some(budget),
            }),
            Err(problem_budget) if problem_budget.code == 8 => {
                let failure = error(
                    8,
                    format!("保留错误超出预算，原始错误code={}", problem.code),
                );
                connection.terminate(&failure.message);
                exhausted(&failure);
                Err(failure)
            }
            Err(problem) => Err(problem),
        }
    }

    /// DOC-16 对象信息：从实际对象取得接口与方法表，结果与额度租约共同持有。
    pub async fn remote_info(
        &self,
        object: &RemoteObject,
        timeout: Duration,
    ) -> Result<Arc<RetainedInfo>> {
        let response = object
            .peer()
            .request(
                "object_info",
                value(&payload::ObjectInfoRequest {
                    object: object.id(),
                })?,
                timeout,
            )
            .await;
        if response.as_ref().is_err_and(|problem| problem.code == 6) {
            object.peer().terminate("object_info 生命周期请求超时");
        }
        let mut info: payload::ObjectInfo = decode(response?.value())?;
        Interface::parse(&info.interface)?;
        info.local = Some(payload::LocalObject {
            process: object.peer().id(),
            object: object.id(),
        });
        self.retain_info(object.peer(), info).map(Arc::new)
    }
}

pub fn strings<'a>(texts: impl IntoIterator<Item = &'a str>) -> Result<usize> {
    texts.into_iter().try_fold(0usize, |total, text| {
        total
            .checked_add(text.len())
            .ok_or_else(|| error(8, "元数据长度超出地址空间"))
    })
}

/// DOC-16 §5：组件元数据只计实际保留的字符串内容。
pub fn component_bytes(component: &payload::Component) -> Result<usize> {
    strings(
        std::iter::once(component.name.as_str())
            .chain(std::iter::once(component.state_schema.as_str()))
            .chain(
                component
                    .inject
                    .iter()
                    .map(|dependency| dependency.interface.as_str()),
            )
            .chain(component.provides.iter().map(String::as_str))
            .chain(component.listens.iter().map(String::as_str)),
    )
}

pub fn descriptor_bytes(descriptor: &payload::Describe) -> Result<usize> {
    descriptor.plugins.iter().try_fold(0usize, |total, plugin| {
        let plugin_bytes = strings([
            plugin.name.as_str(),
            plugin.version.as_str(),
            plugin.sdk_version.as_str(),
            plugin.build_info.as_str(),
        ])?;
        plugin.components.iter().try_fold(
            total
                .checked_add(plugin_bytes)
                .ok_or_else(|| error(8, "元数据长度超出地址空间"))?,
            |total, component| {
                total
                    .checked_add(component_bytes(component)?)
                    .ok_or_else(|| error(8, "元数据长度超出地址空间"))
            },
        )
    })
}

/// CP-3：诊断复合值按现有保留模型计量，节点遍历保持有界 CBOR 的类型。
pub fn value_bytes(value: &Value) -> Result<usize> {
    let mut size = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        let count = match value {
            Value::Text(text) => text.len(),
            Value::Bytes(bytes) => bytes.len(),
            Value::Array(values) => {
                pending.extend(values);
                8
            }
            Value::Map(values) => {
                for (key, value) in values {
                    pending.push(key);
                    pending.push(value);
                }
                8
            }
            Value::Tag(_, value) => {
                pending.push(value);
                8
            }
            _ => 8,
        };
        size = size
            .checked_add(count)
            .ok_or_else(|| error(8, "诊断长度超出地址空间"))?;
    }
    Ok(size)
}
