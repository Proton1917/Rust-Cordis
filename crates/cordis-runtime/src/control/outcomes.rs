//! ADR-0031：按调用方持久化最近批次，响应受理与事务完成分别记录。

use super::*;
use cordis_supervisor::manager::{CallContext, LocalReply};
use cordis_types::{BatchOutcome, BatchTicket};
use serde::{Deserialize, Serialize};

/// ADR-0031：带标签记录保持状态与错误的对应关系，重送时复用真实协议错误。
#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
enum Record {
    Pending { id: String },
    Succeeded { id: String },
    Rejected { id: String, error: WireError },
    Interrupted { id: String, error: WireError },
}

pub(crate) struct BatchOutcomes {
    path: PathBuf,
    records: BTreeMap<String, Record>,
    diagnostics: Arc<Mutex<Diagnostics>>,
}

impl BatchOutcomes {
    /// ADR-0031：路径属于当前项目状态目录，载入在插件启动前执行。
    pub(super) fn new(path: PathBuf, diagnostics: Arc<Mutex<Diagnostics>>) -> Self {
        Self {
            path,
            records: BTreeMap::new(),
            diagnostics,
        }
    }

    /// ADR-0031：核心重启时把残留 pending 转为 interrupted，保留请求身份供重提。
    pub(super) fn load(&mut self) -> Result<()> {
        match std::fs::metadata(&self.path) {
            Ok(_) => {
                self.records =
                    serde_json::from_slice(&crate::storage::read(&self.path)?).map_err(parameter)?
            }
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(problem) => return Err(crate::storage::io_error(problem)),
        }
        let mut changed = false;
        for record in self.records.values_mut() {
            if let Record::Pending { id } = record {
                *record = Record::Interrupted {
                    id: id.clone(),
                    error: WireError::new(7, "核心在批次结果完成前退出"),
                };
                changed = true;
            }
        }
        if changed {
            self.save()?;
        }
        self.publish();
        Ok(())
    }

    /// ADR-0031：保存最近批次的完整状态，容量沿核心帧预算约束。
    fn save(&self) -> Result<()> {
        let data = serde_json::to_vec_pretty(&self.records).map_err(parameter)?;
        if data.len() > cordis_up::Limits::DEFAULT.max_frame {
            return Err(WireError::new(8, "批次结果记录超过状态文件容量"));
        }
        crate::storage::write(&self.path, &data)
    }

    /// ADR-0031：同一请求身份的确定结果保持幂等，中断记录允许重新执行。
    fn replay(&self, caller: &str, request_id: &str) -> Option<Result<Value>> {
        let record = self.records.get(caller)?;
        if record.id() != request_id {
            return None;
        }
        match record {
            Record::Succeeded { .. } => Some(Ok(Value::Null)),
            Record::Rejected { error, .. } => Some(Err(error.clone())),
            Record::Pending { .. } => None,
            Record::Interrupted { .. } => None,
        }
    }

    /// ADR-0031：同一调用方保持单一在途批次，相同身份复用已受理或已完成结果。
    fn known(&self, caller: &str, id: &str) -> Result<bool> {
        match self.records.get(caller) {
            Some(Record::Pending { id: pending }) if pending != id => {
                Err(WireError::new(8, "调用方已有在途批次"))
            }
            Some(Record::Interrupted { .. }) | None => Ok(false),
            Some(record) => Ok(record.id() == id),
        }
    }

    /// ADR-0031：pending 成功持久化后才允许发送 BatchTicket。
    fn begin(&mut self, caller: &str, id: &str) -> Result<()> {
        self.records
            .insert(caller.into(), Record::Pending { id: id.into() });
        let saved = self.save();
        if let Err(problem) = &saved {
            self.records.insert(
                caller.into(),
                Record::Interrupted {
                    id: id.into(),
                    error: problem.clone(),
                },
            );
        }
        self.publish();
        saved
    }

    /// ADR-0031：预约或队列请求退出时只结束仍在 pending 的相同批次。
    fn interrupt(&mut self, caller: &str, id: &str) -> Result<()> {
        if !matches!(self.records.get(caller), Some(Record::Pending { id: pending }) if pending == id)
        {
            return Ok(());
        }
        self.records.insert(
            caller.into(),
            Record::Interrupted {
                id: id.into(),
                error: WireError::new(7, "批次预约或执行在完成前取消"),
            },
        );
        let saved = self.save();
        self.publish();
        saved
    }

    /// ADR-0031：运行状态写出器读取内存中的实际批次状态。
    fn publish(&self) {
        self.diagnostics.lock().last_batch = self.snapshot();
    }

    /// ADR-0031：最终状态由实际执行结果形成，磁盘写入由调用步骤统一处理。
    fn finish(&mut self, caller: &str, id: &str, result: &Result<Value>) {
        let id = id.into();
        self.records.insert(
            caller.into(),
            match result {
                Ok(_) => Record::Succeeded { id },
                Err(error) => Record::Rejected {
                    id,
                    error: error.clone(),
                },
            },
        );
    }

    /// DOC-15 §3.2：公开快照只提供结果信息，协议错误细节保留在核心的幂等记录中。
    fn snapshot(&self) -> Vec<BatchOutcome> {
        self.records
            .iter()
            .map(|(caller, record)| {
                let (state, error) = match record {
                    Record::Pending { .. } => ("pending", None),
                    Record::Succeeded { .. } => ("succeeded", None),
                    Record::Rejected { error, .. } => ("rejected", Some(error.to_string())),
                    Record::Interrupted { error, .. } => ("interrupted", Some(error.to_string())),
                };
                BatchOutcome {
                    id: record.id().into(),
                    caller: caller.clone(),
                    state: state.into(),
                    error,
                }
            })
            .collect()
    }
}

impl Record {
    /// ADR-0031：所有记录状态共享同一请求身份。
    fn id(&self) -> &str {
        match self {
            Self::Pending { id }
            | Self::Succeeded { id }
            | Self::Rejected { id, .. }
            | Self::Interrupted { id, .. } => id,
        }
    }
}

impl Controller {
    /// ADR-0031：持久化 pending 后执行窗口，再持久化最终结果；重送复用确定结果。
    pub(super) async fn execute_recorded_batch(
        &mut self,
        request: &Request,
        id: &str,
    ) -> Result<Value> {
        let caller = &request.caller.instance;
        if let Some(result) = self.outcomes.lock().replay(caller, id) {
            return result;
        }
        let result = self.execute_operation(request).await;
        let mut outcomes = self.outcomes.lock();
        outcomes.finish(caller, id, &result);
        let saved = outcomes.save();
        if let Err(problem) = &saved {
            outcomes.records.insert(
                caller.clone(),
                Record::Interrupted {
                    id: id.into(),
                    error: problem.clone(),
                },
            );
        }
        outcomes.publish();
        saved?;
        result
    }
}

/// ADR-0031、CP-7：响应取消及队列退出沿实际请求所有权结束 pending 记录。
pub(crate) struct PendingBatch {
    outcomes: Arc<Mutex<BatchOutcomes>>,
    caller: String,
    id: String,
}

impl Drop for PendingBatch {
    fn drop(&mut self) {
        if let Err(problem) = self.outcomes.lock().interrupt(&self.caller, &self.id) {
            eprintln!("批次中断记录保存失败：{problem}");
        }
    }
}

impl Management {
    /// ADR-0031：队列预约与 pending 持久化完成后发送受理凭据，响应失败由请求守卫处理。
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
        outcomes.begin(&context.instance, &id)?;
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

    /// ADR-0031：实际队列预约取消后，中断结果写入状态目录并可在下一次启动读取。
    #[test]
    fn cancelled_reservation_persists_interrupted_outcome() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let directory = workspace
            .join("target/runtime-outcomes")
            .join(format!("cancel-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let diagnostics = Arc::new(Mutex::new(Diagnostics::default()));
        let outcomes = Arc::new(Mutex::new(BatchOutcomes::new(
            directory.join("batches.json"),
            diagnostics.clone(),
        )));
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
        outcomes.lock().begin("caller", "cancelled").unwrap();
        let action = action.track_batch(PendingBatch {
            outcomes: outcomes.clone(),
            caller: "caller".into(),
            id: "cancelled".into(),
        });
        drop(action);
        assert!(matches!(
            received.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        let mut restored = BatchOutcomes::new(directory.join("batches.json"), diagnostics);
        restored.load().unwrap();
        assert_eq!(restored.snapshot()[0].state, "interrupted");
        assert!(!restored.known("caller", "cancelled").unwrap());
    }
}
