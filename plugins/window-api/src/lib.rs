//! DOC-15 §11.6：系统 WebView 窗口的通用调用接口。

use cordis_sdk::Result;

#[cordis_sdk::interface(name = "window", version = "1.0")]
pub trait Window: Send + Sync {
    /// PR-1：窗口显示和清理登记完成后交回编号。
    async fn open(&self, url: &str, title: &str, width: u32, height: u32) -> Result<u64>;
    /// PR-1：等待窗口关闭与效应释放，随后发布 closed 通知。
    async fn close(&self, id: u64) -> Result<()>;
}

#[cordis_sdk::event(name = "window.closed", version = "1.0", kind = "async")]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Closed {
    pub id: u64,
}
