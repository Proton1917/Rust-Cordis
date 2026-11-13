//! DOC-15 §11.3、ADR-0038：窗口与 HTTP 插件共享的页面入口接口。
#![forbid(unsafe_code)]

use cordis_sdk::Result;
use serde::{Deserialize, Serialize};

/// DOC-15 §11.6：实际监听地址和本次 HTTP 激活的会话令牌。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebEntry {
    pub url: String,
    pub token: String,
}

/// ADR-0038：窗口通过同一插件接口取得可直接加载的页面地址。
#[cordis_sdk::interface(name = "cordis.web", version = "1.0")]
pub trait Web: Send + Sync {
    fn entry(&self) -> Result<WebEntry>;
}
