//! 可克隆错误与登记位置。

use std::{any::Any, panic::Location, sync::Arc};

/// 内核操作的结果类型，成功值默认为 `()`。
pub type Result<T = ()> = std::result::Result<T, Error>;

/// A-10：错误可向多个等待者分发，并保留效应登记位置。
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    /// 面向调用方的错误说明。
    pub message: Arc<str>,
    /// 传播过程中记录的效应、组件或服务登记位置。
    pub locations: Vec<&'static Location<'static>>,
    /// 聚合失败中的各个成员。
    pub causes: Vec<Error>,
    /// 当前错误的直接原因。
    pub cause: Option<Arc<Error>>,
    /// 配置解析产生的结构化问题及字段路径。
    pub validation: Option<Arc<crate::registry::ValidationError>>,
}

impl Error {
    /// 以给定消息创建错误，关联位置及原因初始为空。
    pub fn new(message: impl Into<Arc<str>>) -> Self {
        Self {
            message: message.into(),
            locations: Vec::new(),
            causes: Vec::new(),
            cause: None,
            validation: None,
        }
    }

    pub(crate) fn at(mut self, location: &'static Location<'static>) -> Self {
        self.locations.push(location);
        self
    }

    /// 设置直接原因并返回当前错误，供逐层传播和日志展开使用。
    pub fn with_cause(mut self, cause: Error) -> Self {
        self.cause = Some(Arc::new(cause));
        self
    }

    pub(crate) fn panic(payload: Box<dyn Any + Send>) -> Self {
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("user callback panicked");
        Self::new(message)
    }

    pub(crate) fn inactive() -> Self {
        Self::new("cannot create effect on inactive context")
    }
}

impl From<crate::registry::ValidationError> for Error {
    fn from(validation: crate::registry::ValidationError) -> Self {
        let mut error = Self::new(validation.to_string());
        error.validation = Some(Arc::new(validation));
        error
    }
}
