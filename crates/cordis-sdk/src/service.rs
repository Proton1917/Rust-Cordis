use crate::Result;
use cordis_abi::{CborValue, Environment, Object};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
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
    /// ADR-0039：调用方保持原句柄，借用包装随参数的调用期使用。
    pub fn borrow(&self) -> BorrowedService<'_, I> {
        BorrowedService {
            value: BorrowedValue::Borrowed(self),
        }
    }

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

impl<I: Interface + ?Sized> fmt::Debug for Service<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Service")
            .field("interface", &I::INTERFACE)
            .finish_non_exhaustive()
    }
}

/// ADR-0039：Serde 聚合通过当前编码事务登记资源，输出仍为协议 tag 60000。
impl<I: Interface + ?Sized> Serialize for Service<I> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let value = cordis_abi::encode_resource(&self.object).map_err(serde::ser::Error::custom)?;
        value.serialize(serializer)
    }
}

impl<'de, I: Interface + ?Sized> Deserialize<'de> for Service<I> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = CborValue::deserialize(deserializer)?;
        let object = cordis_abi::decode_resource(&value).map_err(serde::de::Error::custom)?;
        Self::from_object(object).map_err(serde::de::Error::custom)
    }
}

enum BorrowedValue<'a, I: Interface + ?Sized> {
    Borrowed(&'a Service<I>),
    Imported(Service<I>),
}

/// ADR-0039：发送侧保留原服务的借用期限，接收侧持有本次调用导入的引用。
pub struct BorrowedService<'a, I: Interface + ?Sized> {
    value: BorrowedValue<'a, I>,
}

impl<I: Interface + ?Sized> BorrowedService<'_, I> {
    fn service(&self) -> &Service<I> {
        match &self.value {
            BorrowedValue::Borrowed(service) => service,
            BorrowedValue::Imported(service) => service,
        }
    }
}

impl<I: Interface + ?Sized> Clone for BorrowedService<'_, I> {
    fn clone(&self) -> Self {
        Self {
            value: match &self.value {
                BorrowedValue::Borrowed(service) => BorrowedValue::Borrowed(service),
                BorrowedValue::Imported(service) => BorrowedValue::Imported(service.clone()),
            },
        }
    }
}

impl<I: Interface + ?Sized> Deref for BorrowedService<'_, I> {
    type Target = I;
    fn deref(&self) -> &Self::Target {
        self.service().deref()
    }
}

impl<I: Interface + ?Sized> fmt::Debug for BorrowedService<'_, I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BorrowedService")
            .field("interface", &I::INTERFACE)
            .finish_non_exhaustive()
    }
}

impl<I: Interface + ?Sized> Serialize for BorrowedService<'_, I> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.service().serialize(serializer)
    }
}

impl<'de, I: Interface + ?Sized> Deserialize<'de> for BorrowedService<'_, I> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Service::deserialize(deserializer).map(|service| Self {
            value: BorrowedValue::Imported(service),
        })
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
