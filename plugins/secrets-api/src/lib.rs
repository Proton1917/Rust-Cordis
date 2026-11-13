//! ADR-0046 §3.10：密钥通过服务返回，授权由提供方按受信调用身份判断。
#![forbid(unsafe_code)]

use cordis_sdk::Result;

#[cordis_sdk::interface(name = "cordis.secrets", version = "1.0")]
pub trait Secrets: Send + Sync {
    async fn get(&self, name: String) -> Result<Option<String>>;
}
