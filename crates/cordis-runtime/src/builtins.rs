//! DOC-15 §3.2、PR-7：核心内置管理、运行快照、数据目录与日志接口。

use crate::{
    config::{InstanceSettings, Result, parameter},
    control::BatchOutcomes,
    request::{Handle, Operation},
    status::{Diagnostics, View},
    storage::Store,
};
use cordis_supervisor::manager::{
    Arguments, CallContext, LocalReply, LocalService, LogOrigin, LogRecord, Manager,
};
use cordis_up::{Value, WireError, rpc::HandlerFuture};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Weak},
};

pub type Permissions = Arc<Mutex<BTreeMap<String, InstanceSettings>>>;

pub struct Management {
    pub owner: String,
    pub permissions: Permissions,
    pub control: Handle,
    pub configuration: PathBuf,
    pub store: Store,
    pub diagnostics: Arc<Mutex<Diagnostics>>,
    pub outcomes: Arc<Mutex<BatchOutcomes>>,
    pub logs: Logs,
}

impl LocalService for Management {
    /// DOC-15 §3.2：逐方法授权后分派请求，自变更在响应写出后执行。
    fn invoke<'a>(
        &'a self,
        context: &'a CallContext,
        method: &'a str,
        args: &'a Arguments,
    ) -> HandlerFuture<'a, Result<LocalReply>> {
        Box::pin(async move {
            self.authorize(context, method)?;
            let operation = match Operation::decode(method, &args.values, &self.configuration) {
                Ok(operation) => operation,
                Err(problem) => {
                    if method == "reload" {
                        self.diagnostics.lock().configuration_error = Some(problem.to_string());
                    }
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
            if matches!(&operation, Operation::ReplaceBatch { .. }) {
                return self.batch_reply(context, operation);
            }
            if self.deferred(&operation)? {
                let action = self.control.reserve(context.clone(), operation)?;
                Ok(LocalReply::with_after_write(Value::Null, move || {
                    action.commit()
                }))
            } else {
                self.control
                    .call(context.clone(), operation)
                    .await
                    .map(LocalReply::new)
            }
        })
    }
}

impl Management {
    /// DOC-15 §3.2、ADR-0031：包含配置重读的批次同时需要 replace_batch 和 reload 授权。
    fn authorize(&self, context: &CallContext, method: &str) -> Result<()> {
        if self
            .permissions
            .lock()
            .get(&self.owner)
            .is_some_and(|settings| settings.allows(method))
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

    /// DOC-15 §3.2：自移除、同库替换或授权配置变化在调用方收取响应后提交。
    fn deferred(&self, operation: &Operation) -> Result<bool> {
        Ok(match operation {
            Operation::Shutdown => true,
            Operation::Remove(name)
            | Operation::Replace { name, .. }
            | Operation::Rollback(name)
            | Operation::Configure { name, .. } => name == &self.owner,
            Operation::Reload(configuration) => {
                let target = configuration.effective(&self.store.overlay()?)?;
                target.get(&self.owner) != self.permissions.lock().get(&self.owner)
            }
            Operation::ReplaceBatch { .. } => true,
            _ => false,
        })
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
            let snapshot = self.0.runtime_snapshot().await?;
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
    /// DOC-15 §3.2：插件标准错误原样输出，结构日志进入内置事件接口。
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
