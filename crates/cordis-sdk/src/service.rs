use crate::Result;
use cordis_abi::{CborValue, Environment, Object};
use serde::de::DeserializeOwned;
use std::ops::Deref;
use std::sync::Arc;

/// DOC-12 §7：由 interface 宏生成的接口描述及边界转换。
pub trait Interface: Send + Sync + 'static {
    const INTERFACE: &'static str;
    const MAJOR: u32;
    const MINOR: u32;
    const METHOD_COUNT: usize;
    fn proxy(object: Arc<Object>) -> Result<Arc<Self>>;
    fn object(value: Arc<Self>, environment: Environment) -> Result<Object>;
    fn scoped_object(
        value: Arc<Self>,
        scope: Arc<dyn Fn(CborValue) -> Result<Arc<Self>> + Send + Sync>,
        environment: Environment,
    ) -> Result<Object>;
}

/// ADR-0025：基对象与元数据绑定函数共同构成可按上下文取得视图的提供方。
pub struct Scoped<I: Interface + ?Sized> {
    pub(crate) value: Arc<I>,
    pub(crate) scope: Arc<dyn Fn(CborValue) -> Result<Arc<I>> + Send + Sync>,
}

impl<I: Interface + ?Sized> Scoped<I> {
    pub fn new<M, F>(value: Arc<I>, scope: F) -> Self
    where
        M: DeserializeOwned,
        F: Fn(M) -> Result<Arc<I>> + Send + Sync + 'static,
    {
        Self {
            value,
            scope: Arc::new(move |metadata| scope(cordis_abi::from_value(metadata)?)),
        }
    }
}

/// DOC-12 §11：共享代理的最后一份引用释放时，归还其对象租约。
pub struct Service<I: Interface + ?Sized> {
    proxy: Arc<I>,
    object: Arc<Object>,
}

impl<I: Interface + ?Sized> Service<I> {
    #[doc(hidden)]
    pub fn from_object(object: Object) -> Result<Self> {
        let object = Arc::new(object);
        Ok(Self {
            proxy: I::proxy(object.clone())?,
            object,
        })
    }

    #[doc(hidden)]
    pub fn clone_object(&self) -> Result<Object> {
        self.object.try_clone()
    }
}

impl<I: Interface + ?Sized> Clone for Service<I> {
    fn clone(&self) -> Self {
        Self {
            proxy: self.proxy.clone(),
            object: self.object.clone(),
        }
    }
}

impl<I: Interface + ?Sized> Deref for Service<I> {
    type Target = I;

    fn deref(&self) -> &Self::Target {
        self.proxy.as_ref()
    }
}
