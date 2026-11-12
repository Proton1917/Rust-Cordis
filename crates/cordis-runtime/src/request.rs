//! DOC-15 §3.2、ADR-0031：管理参数解码、有界串行队列与响应后提交。

use crate::{
    config::{self, Configuration, Result, parameter},
    storage,
};
use cordis_supervisor::manager::CallContext;
use cordis_up::{Value, WireError};
use serde::de::DeserializeOwned;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

pub enum Operation {
    Install(String),
    Add {
        name: String,
        plugin: String,
        group: Option<String>,
        config: toml::Table,
    },
    Remove(String),
    Replace {
        name: String,
        plugin: String,
    },
    ReplaceBatch {
        replacements: Vec<(String, String)>,
        reload: bool,
        configuration: Result<Option<Configuration>>,
        request_id: String,
    },
    Rollback(String),
    Configure {
        name: String,
        config: toml::Table,
    },
    Reload(Configuration),
    Shutdown,
}

impl Operation {
    /// DOC-16 §5、ADR-0031：方法参数按共享 CBOR 值解码，reload 请求固定该窗口配置。
    pub fn decode(method: &str, values: &[Value], configuration: &Path) -> Result<Self> {
        fn args<T: DeserializeOwned>(values: &[Value]) -> Result<T> {
            Value::Array(values.to_vec())
                .deserialized()
                .map_err(parameter)
        }
        Ok(match method {
            "install" => {
                let (source,): (String,) = args(values)?;
                Self::Install(source)
            }
            "add" => {
                let (name, plugin, group, config) = args(values)?;
                Self::Add {
                    name,
                    plugin,
                    group,
                    config,
                }
            }
            "remove" => {
                let (name,): (String,) = args(values)?;
                Self::Remove(name)
            }
            "replace" => {
                let (name, plugin) = args(values)?;
                Self::Replace { name, plugin }
            }
            "replace_batch" => {
                let (replacements, reload, request_id): (Vec<(String, String)>, bool, String) =
                    args(values)?;
                let configuration = reload
                    .then(|| load_configuration(configuration))
                    .transpose();
                Self::ReplaceBatch {
                    replacements,
                    reload,
                    configuration,
                    request_id,
                }
            }
            "rollback" => {
                let (name,): (String,) = args(values)?;
                Self::Rollback(name)
            }
            "configure" => {
                let (name, config) = args(values)?;
                Self::Configure { name, config }
            }
            "reload" | "shutdown" => {
                if !values.is_empty() {
                    return Err(parameter("该方法接受空参数数组"));
                }
                if method == "shutdown" {
                    Self::Shutdown
                } else {
                    Self::Reload(load_configuration(configuration)?)
                }
            }
            _ => return Err(WireError::new(3, "管理方法缺席")),
        })
    }

    /// DOC-15 §3.2：审计与授权共用操作方法名。
    pub fn method(&self) -> &'static str {
        match self {
            Self::Install(_) => "install",
            Self::Add { .. } => "add",
            Self::Remove(_) => "remove",
            Self::Replace { .. } => "replace",
            Self::ReplaceBatch { .. } => "replace_batch",
            Self::Rollback(_) => "rollback",
            Self::Configure { .. } => "configure",
            Self::Reload(_) => "reload",
            Self::Shutdown => "shutdown",
        }
    }

    /// DOC-15 §3.2：单实例操作记录名称，联合部署操作归属整个运行时。
    pub fn target(&self) -> &str {
        match self {
            Self::Install(source) => source,
            Self::Remove(name) | Self::Rollback(name) => name,
            Self::Add { name, .. } | Self::Replace { name, .. } | Self::Configure { name, .. } => {
                name
            }
            Self::ReplaceBatch { .. } | Self::Reload(_) | Self::Shutdown => "runtime",
        }
    }
}

pub struct Request {
    pub caller: CallContext,
    pub operation: Operation,
    pub reply: Option<oneshot::Sender<Result<Value>>>,
    pending_batch: Option<crate::control::PendingBatch>,
}

#[derive(Clone)]
pub struct Handle {
    sender: mpsc::Sender<Request>,
    accepting: Arc<AtomicBool>,
    timeout: Duration,
}

pub struct Reserved {
    permit: mpsc::OwnedPermit<Request>,
    request: Request,
}
impl Reserved {
    /// ADR-0031：预约和排队请求共同持有未完成批次，取消时由守卫保存 interrupted。
    pub(crate) fn track_batch(mut self, pending: crate::control::PendingBatch) -> Self {
        self.request.pending_batch = Some(pending);
        self
    }

    /// DOC-15 §3.2：响应写入完成后消费预留位置，队列继续拥有操作执行责任。
    pub fn commit(self) {
        self.permit.send(self.request);
    }
}

impl Handle {
    /// CP-3/CP-4：固定容量管理队列和调用方等待时限。
    pub fn channel(timeout: Duration) -> (Self, mpsc::Receiver<Request>) {
        let (sender, receiver) = mpsc::channel(64);
        (
            Self {
                sender,
                accepting: Arc::new(AtomicBool::new(true)),
                timeout,
            },
            receiver,
        )
    }

    /// CP-3：入口只预留一次队列容量，关闭和饱和分别返回实际状态。
    fn prepare(
        &self,
        caller: CallContext,
        operation: Operation,
        reply: Option<oneshot::Sender<Result<Value>>>,
    ) -> Result<Reserved> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(WireError::new(7, "核心正在退出"));
        }
        let permit = self
            .sender
            .clone()
            .try_reserve_owned()
            .map_err(|problem| match problem {
                mpsc::error::TrySendError::Full(_) => WireError::new(8, "管理操作队列已满"),
                mpsc::error::TrySendError::Closed(_) => WireError::new(7, "管理操作已结束"),
            })?;
        Ok(Reserved {
            permit,
            request: Request {
                caller,
                operation,
                reply,
                pending_batch: None,
            },
        })
    }

    /// DOC-15 §3.2：自变更先返回调用响应，再交给队列执行。
    pub fn reserve(&self, caller: CallContext, operation: Operation) -> Result<Reserved> {
        self.prepare(caller, operation, None)
    }

    /// CP-4、A-7：等待者超时结束等待，已提交事务由接收端继续执行。
    pub async fn call(&self, caller: CallContext, operation: Operation) -> Result<Value> {
        let (reply, receiver) = oneshot::channel();
        self.prepare(caller, operation, Some(reply))?.commit();
        tokio::time::timeout(self.timeout, receiver)
            .await
            .map_err(|_| WireError::new(6, "管理操作等待截止时间到达"))?
            .map_err(|_| WireError::new(7, "管理操作已结束"))?
    }

    /// DOC-11 §9：退出阶段关闭新请求入口，现有队列由运行循环处理。
    pub fn stop(&self) {
        self.accepting.store(false, Ordering::Release);
    }
}

/// DOC-15 §3.2：读取本窗口配置并检查核心版本要求，格式和版本错误保持来源。
fn load_configuration(path: &Path) -> Result<Configuration> {
    let configuration = config::parse(&storage::read(path)?)?;
    configuration
        .check_version(&semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(parameter)?)?;
    Ok(configuration)
}
