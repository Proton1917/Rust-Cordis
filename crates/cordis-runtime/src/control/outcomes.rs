//! ADR-0031：按调用方保存最近一次批次的结果，供调用方通过运行时快照读取。
//!
//! 结果保存在内存中。同一调用方最近一项请求的重复投递返回已有受理票据；
//! 核心重启后调用方通过原请求标识重新提交已安装引用。

use super::*;
use cordis_supervisor::manager::{CallContext, LocalReply};
use cordis_types::{BatchOutcome, BatchTicket};

enum Record {
    Pending,
    Succeeded,
    Rejected {
        error: WireError,
        discarded_confirmation: Option<cordis_types::ConfirmationDiscard>,
    },
}

pub(crate) struct BatchOutcomes {
    /// 调用方实例名 → (请求标识, 结果)。
    records: BTreeMap<String, (String, Record)>,
    diagnostics: Arc<Mutex<Diagnostics>>,
}

impl BatchOutcomes {
    pub(super) fn new(diagnostics: Arc<Mutex<Diagnostics>>) -> Self {
        Self {
            records: BTreeMap::new(),
            diagnostics,
        }
    }

    /// 同一调用方保持一个在途批次，相同标识的重发沿用最近结果。
    fn known(&self, caller: &str, id: &str) -> Result<bool> {
        match self.records.get(caller) {
            Some((pending, Record::Pending)) if pending != id => {
                Err(WireError::new(8, "调用方已有在途批次"))
            }
            Some((known, _)) => Ok(known == id),
            _ => Ok(false),
        }
    }

    fn set(&mut self, caller: &str, id: &str, record: Record) {
        self.records.insert(caller.into(), (id.into(), record));
        self.diagnostics.lock().last_batch = self.snapshot();
    }

    /// 请求在执行前被取消时，仍处于 pending 的同一批次记为被拒绝。
    fn cancel(&mut self, caller: &str, id: &str) {
        if matches!(self.records.get(caller), Some((pending, Record::Pending)) if pending == id) {
            self.set(
                caller,
                id,
                Record::Rejected {
                    error: WireError::new(7, "批次在执行前被取消"),
                    discarded_confirmation: None,
                },
            );
        }
    }

    fn snapshot(&self) -> Vec<BatchOutcome> {
        self.records
            .iter()
            .map(|(caller, (id, record))| {
                let (state, error) = match record {
                    Record::Pending => ("pending", None),
                    Record::Succeeded => ("succeeded", None),
                    Record::Rejected { error, .. } => ("rejected", Some(error)),
                };
                let discarded_confirmation = match record {
                    Record::Rejected {
                        discarded_confirmation,
                        ..
                    } => discarded_confirmation.clone(),
                    _ => None,
                };
                BatchOutcome {
                    id: id.clone(),
                    caller: caller.clone(),
                    state: state.into(),
                    error: error.map(ToString::to_string),
                    code: error.map(|error| error.code),
                    discarded_confirmation,
                }
            })
            .collect()
    }
}

impl Controller {
    /// 执行批次并记录结果。
    pub(super) async fn execute_recorded_batch(
        &mut self,
        request: &Request,
        id: &str,
    ) -> Result<Value> {
        self.batch_discarded = None;
        let result = self.execute_operation(request).await;
        let record = match &result {
            Ok(_) => Record::Succeeded,
            Err(error) => Record::Rejected {
                error: error.clone(),
                discarded_confirmation: self.batch_discarded.take(),
            },
        };
        self.outcomes
            .lock()
            .set(&request.caller.instance, id, record);
        result
    }
}

/// 预约或排队中的批次被丢弃（响应写出失败、核心退出）时，把 pending 记录结束为被拒绝。
pub(crate) struct PendingBatch {
    outcomes: Arc<Mutex<BatchOutcomes>>,
    caller: String,
    id: String,
}

impl Drop for PendingBatch {
    fn drop(&mut self) {
        self.outcomes.lock().cancel(&self.caller, &self.id);
    }
}

impl Management {
    /// 批次先返回受理票据，在响应写出后进入管理队列执行；调用方从运行时快照读取结果。
    ///
    /// 票据的作用：调用方（如 watch）可能在本批次中被替换，不能同步等待批次结束。
    pub(crate) fn batch_reply(
        &self,
        context: &CallContext,
        operation: Operation,
    ) -> Result<LocalReply> {
        let Operation::ReplaceBatch { request_id, .. } = &operation else {
            unreachable!("batch operation selected")
        };
        let id = request_id.clone();
        let ticket = Value::serialized(&BatchTicket { id: id.clone() }).map_err(parameter)?;
        let mut outcomes = self.outcomes.lock();
        if outcomes.known(&context.instance, &id)? {
            return Ok(LocalReply::new(ticket));
        }
        let action = self.control.reserve(context.clone(), operation)?;
        outcomes.set(&context.instance, &id, Record::Pending);
        drop(outcomes);
        let action = action.track_batch(PendingBatch {
            outcomes: self.outcomes.clone(),
            caller: context.instance.clone(),
            id,
        });
        Ok(LocalReply::with_after_write(ticket, move || {
            action.commit()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 预约被丢弃时，pending 批次记为 code 7 的拒绝，调用方据此重新提交。
    #[test]
    fn cancelled_reservation_rejects_pending_batch() {
        let diagnostics = Arc::new(Mutex::new(Diagnostics::default()));
        let outcomes = Arc::new(Mutex::new(BatchOutcomes::new(diagnostics.clone())));
        let (handle, mut received) = Handle::channel(Duration::from_secs(1));
        let action = handle
            .reserve(
                CallContext {
                    connection: 1,
                    context: 1,
                    instance: "caller".into(),
                    group: "caller".into(),
                    plugin: "caller".into(),
                },
                Operation::ReplaceBatch {
                    replacements: Vec::new(),
                    reload: false,
                    configuration: Ok(None),
                    request_id: "cancelled".into(),
                },
            )
            .unwrap();
        outcomes.lock().set("caller", "cancelled", Record::Pending);
        assert!(outcomes.lock().known("caller", "cancelled").unwrap());
        assert!(outcomes.lock().known("caller", "other").is_err());
        drop(action.track_batch(PendingBatch {
            outcomes: outcomes.clone(),
            caller: "caller".into(),
            id: "cancelled".into(),
        }));
        assert!(matches!(
            received.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        let last = &diagnostics.lock().last_batch[0];
        assert_eq!(last.state, "rejected");
        assert!(outcomes.lock().known("caller", "cancelled").unwrap());
    }
}
