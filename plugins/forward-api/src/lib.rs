//! DOC-16 §11、ADR-0041：原生转发方通过既有服务 ABI 操作独立页面通道。
#![forbid(unsafe_code)]

use cordis_sdk::{CborValue, Result, Service};

#[cordis_sdk::interface(name = "cordis.forward-receiver", version = "1.0")]
pub trait ForwardReceiver: Send + Sync {
    async fn frame(&self, frame: Vec<u8>) -> Result<()>;
    async fn close(&self, reason: String) -> Result<()>;
}

#[cordis_sdk::interface(name = "cordis.forward", version = "1.0")]
pub trait Forward: Send + Sync {
    async fn open(
        &self,
        metadata: CborValue,
        receiver: Service<dyn ForwardReceiver>,
    ) -> Result<u64>;
    async fn frame(&self, channel: u64, frame: Vec<u8>) -> Result<()>;
    async fn close(&self, channel: u64, reason: String) -> Result<()>;
}
