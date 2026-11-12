//! DOC-15 CP-3、CP-8：连接保留的描述、元数据与错误受共享额度约束。
//!
//! 边界登记时计量并取得租约，最后引用释放时归还额度；诊断文本按固定容量截断。

use super::*;
use std::sync::atomic::AtomicUsize;

pub(super) const LOG_BYTES: usize = 4096;

pub(super) struct MetadataLease {
    counter: Arc<AtomicUsize>,
    bytes: usize,
}
impl Drop for MetadataLease {
    fn drop(&mut self) {
        self.counter.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

pub(super) struct RetainedInfo {
    pub value: payload::ObjectInfo,
    pub _budget: Option<Arc<MetadataLease>>,
}
pub(super) struct RetainedError {
    pub value: WireError,
    pub _budget: Option<Arc<MetadataLease>>,
}

pub(super) fn clipped(text: &str) -> String {
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

pub(super) fn saved_error(problem: &WireError) -> WireError {
    WireError {
        code: problem.code,
        message: clipped(&problem.message),
        detail: Value::Null,
    }
}

pub(super) fn strings<'a>(texts: impl IntoIterator<Item = &'a str>) -> Result<usize> {
    texts.into_iter().try_fold(0usize, |total, text| {
        total
            .checked_add(text.len())
            .ok_or_else(|| error(8, "元数据长度超出地址空间"))
    })
}

pub(super) fn component_bytes(component: &payload::Component) -> Result<usize> {
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

pub(super) fn descriptor_bytes(descriptor: &payload::Describe) -> Result<usize> {
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

pub(super) fn value_bytes(value: &Value) -> Result<usize> {
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

impl Manager {
    /// DOC-15 CP-3：活跃连接的保留字节在登记时取得额度，租约 Drop 归还额度。
    pub(super) fn retain_metadata(
        &self,
        connection: &Connection,
        bytes: usize,
    ) -> Result<Arc<MetadataLease>> {
        let counter = {
            let mut counters = self.0.metadata.lock();
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
                used.checked_add(bytes)
                    .filter(|next| *next <= self.0.options.max_retained_metadata_bytes)
            })
            .map_err(|_| error(8, "该连接的保留元数据内容已达上限"))?;
        Ok(Arc::new(MetadataLease { counter, bytes }))
    }

    /// DOC-16 对象信息：方法名与接口名共同计入连接额度，失败保持原登记状态。
    pub(super) fn retain_info(
        &self,
        connection: &Connection,
        info: payload::ObjectInfo,
    ) -> Result<RetainedInfo> {
        let budget = self.retain_metadata(
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

    /// DOC-15 CP-3、CP-6：保留真实错误；额度耗尽时结束连接并保存有界诊断。
    pub(super) fn retain_error(
        &self,
        connection: &Connection,
        problem: &WireError,
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
        match self.retain_metadata(connection, bytes) {
            Ok(budget) => Ok(RetainedError {
                value: problem.clone(),
                _budget: Some(budget),
            }),
            Err(problem_budget) if problem_budget.code == 8 => {
                let exhausted = error(
                    8,
                    format!("保留错误超出预算，原始错误code={}", problem.code),
                );
                connection.terminate(&exhausted.message);
                self.0.context.logger().error(exhausted.to_string());
                Err(exhausted)
            }
            Err(problem) => Err(problem),
        }
    }
}
