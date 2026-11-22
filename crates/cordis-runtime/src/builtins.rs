//! DOC-15 §3.2、PR-7：核心内置管理、运行快照、数据目录与日志接口。

use crate::{
    config::{Result, parameter},
    control::BatchOutcomes,
    request::{Handle, Operation},
    status::View,
    storage::Store,
};
use cordis_supervisor::manager::{
    Arguments, CallContext, LocalReply, LocalService, LogOrigin, LogRecord, Manager,
};
use cordis_up::{Value, WireError, rpc::HandlerFuture};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{Arc, Weak},
};

pub struct Management {
    pub legacy: bool,
    pub grants: Vec<String>,
    pub control: Handle,
    pub configuration: PathBuf,
    pub outcomes: Arc<Mutex<BatchOutcomes>>,
    pub logs: Logs,
}

impl LocalService for Management {
    /// 安装返回已固定的产物引用；结构变更在票据写出后交给队列。
    fn invoke<'a>(
        &'a self,
        context: &'a CallContext,
        method: &'a str,
        args: &'a Arguments,
    ) -> HandlerFuture<'a, Result<LocalReply>> {
        Box::pin(async move {
            self.authorize(context, method)?;
            if !self.legacy && method == "result" {
                let [Value::Text(id)] = args.values.as_slice() else {
                    return Err(parameter("result 需要一个票据标识"));
                };
                let records = self.outcomes.lock();
                return Value::serialized(&records.get(&context.instance, id))
                    .map(LocalReply::new)
                    .map_err(parameter);
            }
            if self.legacy && !matches!(method, "install" | "replace_batch" | "shutdown") {
                return Err(WireError::new(
                    3,
                    "该结构变更使用 cordis.manage@2.0 票据接口",
                ));
            }
            let operation = match Operation::decode(method, &args.values, &self.configuration) {
                Ok(operation) => operation,
                Err(problem) => {
                    self.logs.emit(
                        0,
                        &context.instance,
                        &format!("manage method={method} result=error {}", problem),
                    );
                    return Err(problem);
                }
            };
            if matches!(&operation, Operation::ReplaceBatch { reload: true, .. }) {
                self.authorize(context, "reload")?;
            }
            if matches!(operation, Operation::Install(_)) {
                return self
                    .control
                    .call(context.clone(), operation)
                    .await
                    .map(LocalReply::new);
            }
            let shutdown = matches!(operation, Operation::Shutdown);
            let mut reply = self.ticket_reply(context, operation)?;
            if self.legacy && shutdown {
                reply.value = Value::Null;
            }
            Ok(reply)
        })
    }
}

impl Management {
    /// DOC-15 §3.2、ADR-0031：包含配置重读的批次同时需要 replace_batch 和 reload 授权。
    fn authorize(&self, context: &CallContext, method: &str) -> Result<()> {
        if (!self.legacy && method == "result")
            || self
                .grants
                .iter()
                .any(|allowed| allowed == "*" || allowed == method)
        {
            return Ok(());
        }
        self.logs.emit(
            1,
            &context.instance,
            &format!("manage method={method} result=error 9"),
        );
        Err(WireError::new(
            9,
            format!("实例 {} 缺少管理方法 {method} 的授权", context.instance),
        ))
    }
}

pub struct RuntimeInfo(pub View);
impl LocalService for RuntimeInfo {
    /// DOC-15 §3.2：运行接口提供只读快照，参数错误在服务边界返回。
    fn invoke<'a>(
        &'a self,
        _context: &'a CallContext,
        method: &'a str,
        args: &'a Arguments,
    ) -> HandlerFuture<'a, Result<LocalReply>> {
        Box::pin(async move {
            if method != "snapshot" {
                return Err(WireError::new(3, "运行状态方法缺席"));
            }
            if !args.values.is_empty() {
                return Err(parameter("该方法接受空参数数组"));
            }
            let mut snapshot = self.0.runtime_snapshot()?;
            // 运行快照 1.x 保留已发布的状态值，管理接口 2.0 直接返回详细终态。
            for outcome in &mut snapshot.last_batch {
                outcome.state = match outcome.state.as_str() {
                    "committed_with_cleanup" => "succeeded".into(),
                    "restored" | "recovery_required" => "rejected".into(),
                    _ => continue,
                };
            }
            Value::serialized(&snapshot)
                .map(LocalReply::new)
                .map_err(parameter)
        })
    }
}

pub struct DataDirectory(pub Store);
impl LocalService for DataDirectory {
    /// PR-7：使用调用方的真实逻辑插件身份分配数据目录。
    fn invoke<'a>(
        &'a self,
        context: &'a CallContext,
        method: &'a str,
        args: &'a Arguments,
    ) -> HandlerFuture<'a, Result<LocalReply>> {
        Box::pin(async move {
            if method != "path" {
                return Err(WireError::new(3, "数据目录方法缺席"));
            }
            if !args.values.is_empty() {
                return Err(parameter("该方法接受空参数数组"));
            }
            let directory = self.0.data_dir(&context.plugin)?;
            Ok(LocalReply::new(
                directory.to_string_lossy().into_owned().into(),
            ))
        })
    }
}

#[derive(Serialize)]
struct EventRecord<'a> {
    level: u32,
    message: &'a str,
    instance: &'a str,
}

#[derive(Clone)]
pub struct Logs(pub Weak<Manager>);
impl Logs {
    /// DOC-15 §3.2：插件标准错误经有界处理后输出，结构日志进入内置事件接口。
    pub fn record(&self, record: LogRecord) {
        if record.origin == LogOrigin::Stderr {
            eprintln!("[{}] {}", record.instance, record.message);
        } else {
            self.emit(record.level, &record.instance, &record.message);
        }
    }

    /// ADR-0017：日志以 UP/1 数据事件派发，发送失败报告至标准错误。
    pub fn emit(&self, level: u32, instance: &str, message: &str) {
        if let Some(manager) = self.0.upgrade() {
            let result = Value::serialized(&EventRecord {
                level,
                instance,
                message,
            })
            .map_err(parameter)
            .and_then(|value| manager.emit_builtin("cordis.log@1.0", value));
            if let Err(problem) = result {
                eprintln!("日志事件派发失败：{problem}");
            }
        }
    }
}
