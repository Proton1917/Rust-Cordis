//! ADR-0047：按名称调用既有 V-2 入口，对象租约和资源编解码沿 ABI 的拥有关系。

use crate::{CborValue, Error, Result};
use cordis_abi::{Buf, CborEncoder, Environment, Object, decode_in};
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;

#[derive(Clone)]
pub struct DynamicService {
    object: Arc<Object>,
}

impl DynamicService {
    pub(crate) fn from_object(object: Object) -> Self {
        Self {
            object: Arc::new(object),
        }
    }

    /// ADR-0047 §3.7：原始调用传递纯 CBOR 值，资源由 invoke_as 的 Service 类型承接。
    pub fn invoke(&self, method: &str, args: CborValue) -> Result<CborValue> {
        plain(&args)?;
        let value = self.invoke_as(method, &args)?;
        plain(&value)?;
        Ok(value)
    }

    /// ADR-0047 §3.7：参数及结果的资源标签返回参数错误，结果缓冲先完成释放。
    pub async fn invoke_async(&self, method: &str, args: CborValue) -> Result<CborValue> {
        plain(&args)?;
        let value = self.invoke_as_async(method, &args).await?;
        plain(&value)?;
        Ok(value)
    }

    /// ADR-0047 §3：位置参数以 tuple 或列表编码；资源字段沿调用期环境导出与导入。
    pub fn invoke_as<A: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        args: &A,
    ) -> Result<R> {
        let environment = self.object.environment();
        let arguments = CborEncoder::new(environment).encode(args)?;
        let response = self.object.invoke(method, arguments.as_slice())?;
        arguments.release()?;
        decode(response, environment)
    }

    /// ADR-0047 §3、DOC-12 §8：异步入口接管参数后归还缓冲，取消沿既有 Future 释放。
    pub async fn invoke_as_async<A: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: &str,
        args: &A,
    ) -> Result<R> {
        let environment = self.object.environment();
        let arguments = CborEncoder::new(environment).encode(args)?;
        let future = self.object.invoke_async(method, arguments.as_slice())?;
        arguments.release()?;
        decode(future.await?, environment)
    }
}

fn plain(value: &CborValue) -> Result<()> {
    match value {
        CborValue::Tag(60000, _) => {
            return Err(Error::new(
                2,
                "资源参数和结果请使用 invoke_as 或 invoke_as_async",
            ));
        }
        CborValue::Tag(_, inner) => plain(inner)?,
        CborValue::Array(values) => {
            for value in values {
                plain(value)?;
            }
        }
        CborValue::Map(fields) => {
            for (key, value) in fields {
                plain(key)?;
                plain(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn decode<R: DeserializeOwned>(response: Buf, environment: Environment) -> Result<R> {
    let value = decode_in(response.as_slice(), environment)?;
    response.release()?;
    Ok(value)
}
