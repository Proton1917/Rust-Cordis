//! 管理操作先受理，响应写出后由有界队列执行。结果按真实调用实例和票据查询。

use super::*;
use cordis_supervisor::manager::{CallContext, LocalReply};
use cordis_types::{BatchOutcome, BatchTicket, ConfirmationDiscard};
use std::collections::VecDeque;

#[derive(Default)]
pub(crate) struct BatchOutcomes {
    records: VecDeque<BatchOutcome>,
    sequence: u64,
}

impl BatchOutcomes {
    pub(crate) fn snapshot(&self) -> Vec<BatchOutcome> {
        self.records.iter().rev().cloned().collect()
    }

    pub(crate) fn end_pending(&mut self) {
        for row in self.records.iter_mut().filter(|row| row.state == "pending") {
            row.state = "rejected".into();
            row.code = Some(7);
            row.error = Some("核心退出，排队操作已取消".into());
        }
    }

    pub(crate) fn get(&self, caller: &str, id: &str) -> Option<&BatchOutcome> {
        self.records
            .iter()
            .find(|row| row.caller == caller && row.id == id)
    }

    fn accept(&mut self, caller: &str, id: String) -> Result<()> {
        if self.records.len() == crate::request::CAPACITY {
            let index = self
                .records
                .iter()
                .position(|row| row.state != "pending")
                .ok_or_else(|| WireError::new(8, "管理票据容量已满"))?;
            self.records.remove(index);
        }
        self.records.push_back(BatchOutcome {
            caller: caller.to_owned(),
            id,
            state: "pending".into(),
            error: None,
            code: None,
            discarded_confirmation: None,
        });
        Ok(())
    }
}

/// 响应、预约和已提交请求依次拥有此票据。每个终态消费一次守卫。
pub(crate) struct PendingBatch {
    outcomes: Arc<Mutex<BatchOutcomes>>,
    caller: String,
    id: String,
    armed: bool,
}

impl PendingBatch {
    pub(crate) fn finish(
        mut self,
        state: &str,
        problem: Option<&WireError>,
        discarded: Option<ConfirmationDiscard>,
    ) {
        self.record(state, problem, discarded);
        self.armed = false;
    }

    fn record(
        &self,
        state: &str,
        problem: Option<&WireError>,
        discarded: Option<ConfirmationDiscard>,
    ) {
        let mut outcomes = self.outcomes.lock();
        if let Some(row) = outcomes
            .records
            .iter_mut()
            .find(|r| r.caller == self.caller && r.id == self.id && r.state == "pending")
        {
            row.state = state.into();
            row.error = problem.map(ToString::to_string);
            row.code = problem.map(|problem| problem.code);
            row.discarded_confirmation = discarded;
        }
    }
}

impl Drop for PendingBatch {
    fn drop(&mut self) {
        if self.armed {
            self.record(
                "rejected",
                Some(&WireError::new(7, "管理请求在执行前被取消")),
                None,
            );
        }
    }
}

impl Management {
    pub(crate) fn ticket_reply(
        &self,
        context: &CallContext,
        operation: Operation,
    ) -> Result<LocalReply> {
        let mut outcomes = self.outcomes.lock();
        let id = if let Operation::ReplaceBatch { request_id, .. } = &operation {
            if request_id.is_empty() || request_id.len() > 256 {
                return Err(parameter("批次请求标识需要 1 至 256 个 UTF-8 字节"));
            }
            if outcomes.get(&context.instance, request_id).is_some() {
                let ticket = BatchTicket {
                    id: request_id.clone(),
                };
                return Ok(LocalReply::new(
                    Value::serialized(&ticket).map_err(parameter)?,
                ));
            }
            request_id.clone()
        } else {
            loop {
                outcomes.sequence = outcomes
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| parameter("管理票据序号耗尽"))?;
                let id = format!("cordis-{}", outcomes.sequence);
                if outcomes.get(&context.instance, &id).is_none() {
                    break id;
                }
            }
        };
        let ticket = Value::serialized(&BatchTicket { id: id.clone() }).map_err(parameter)?;
        let action = self.control.reserve(context.clone(), operation)?;
        outcomes.accept(&context.instance, id.clone())?;
        drop(outcomes);
        let action = action.track_batch(PendingBatch {
            outcomes: self.outcomes.clone(),
            caller: context.instance.clone(),
            id,
            armed: true,
        });
        Ok(LocalReply::with_after_write(ticket, move || {
            action.commit()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_reservation_rejects_its_ticket_without_running_the_operation() {
        let outcomes = Arc::new(Mutex::new(BatchOutcomes::default()));
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
                Operation::Remove("obsolete".into()),
            )
            .unwrap();
        let id = "request-17".to_owned();
        outcomes.lock().accept("caller", id.clone()).unwrap();
        drop(action.track_batch(PendingBatch {
            outcomes: outcomes.clone(),
            caller: "caller".into(),
            id,
            armed: true,
        }));
        assert!(matches!(
            received.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
        let records = outcomes.lock();
        let outcome = records.get("caller", "request-17").unwrap();
        assert_eq!(
            (outcome.state.as_str(), outcome.code),
            ("rejected", Some(7))
        );
        assert!(records.get("unrelated", "request-17").is_none());
    }
}
