use crate::Result;
use cordis_abi::{Environment, Object};
use std::ops::Deref;
use std::sync::Arc;

/// DOC-12 §7：由 interface 宏生成的接口描述及边界转换。
pub trait Interface: Send + Sync + 'static {
    const INTERFACE: &'static str;
    const MAJOR: u32;
    const MINOR: u32;
    const METHOD_COUNT: usize;
    type Proxy: Proxy<Self>;

    fn proxy(object: Object) -> Result<Self::Proxy>;
    fn object(value: Arc<Self>, environment: Environment) -> Result<Object>;
}

#[doc(hidden)]
pub trait Proxy<I: ?Sized>: AsRef<I> + Send + Sync + 'static {
    fn object(&self) -> &Object;
}

/// DOC-12 §11：共享代理的最后一份引用释放时，归还其对象租约。
pub struct Service<I: Interface + ?Sized> {
    proxy: Arc<I::Proxy>,
}

impl<I: Interface + ?Sized> Service<I> {
    #[doc(hidden)]
    pub fn from_object(object: Object) -> Result<Self> {
        Ok(Self {
            proxy: Arc::new(I::proxy(object)?),
        })
    }

    #[doc(hidden)]
    pub fn clone_object(&self) -> Result<Object> {
        self.proxy.object().try_clone()
    }
}

impl<I: Interface + ?Sized> Clone for Service<I> {
    fn clone(&self) -> Self {
        Self {
            proxy: self.proxy.clone(),
        }
    }
}

impl<I: Interface + ?Sized> Deref for Service<I> {
    type Target = I;

    fn deref(&self) -> &Self::Target {
        self.proxy.as_ref().as_ref()
    }
}
