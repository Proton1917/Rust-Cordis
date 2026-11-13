//! DOC-15 CP-3、CP-8：连接保留的描述、元数据与错误受共享额度约束。
//!
//! 边界登记时计量并取得租约，最后引用释放时归还额度；诊断文本按固定容量截断。

use super::*;
pub(super) use cordis_routing::metadata::{
    LOG_BYTES, MetadataLease, RetainedError, RetainedInfo, clipped, component_bytes,
    descriptor_bytes, saved_error, strings, value_bytes,
};

impl Manager {
    /// DOC-15 CP-3：活跃连接的保留字节在登记时取得额度，租约 Drop 归还额度。
    pub(super) fn retain_metadata(
        &self,
        connection: &Connection,
        bytes: usize,
    ) -> Result<Arc<MetadataLease>> {
        self.0.metadata.retain(connection, bytes)
    }

    /// DOC-16 对象信息：方法名与接口名共同计入连接额度，失败保持原登记状态。
    pub(super) fn retain_info(
        &self,
        connection: &Connection,
        info: payload::ObjectInfo,
    ) -> Result<RetainedInfo> {
        self.0.metadata.retain_info(connection, info)
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
