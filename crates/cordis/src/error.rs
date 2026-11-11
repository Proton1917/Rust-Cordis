//! 可克隆错误与登记位置。

use std::{any::Any, panic::Location, sync::Arc};

pub type Result<T = ()> = std::result::Result<T, Error>;

/// A-10：错误可向多个等待者分发，并保留效应登记位置。
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub message: Arc<str>,
    pub locations: Vec<&'static Location<'static>>,
    pub causes: Vec<Error>,
    pub validation: Option<Arc<crate::registry::ValidationError>>,
}

impl Error {
    pub fn new(message: impl Into<Arc<str>>) -> Self {
        Self {
            message: message.into(),
            locations: Vec::new(),
            causes: Vec::new(),
            validation: None,
        }
    }

    pub(crate) fn at(mut self, location: &'static Location<'static>) -> Self {
        self.locations.push(location);
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
