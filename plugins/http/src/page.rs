//! ADR-0046 §3.9：升级请求属性由 HTTP 进程保存，连接结束时撤回随机页面键。

use axum::http::{HeaderMap, StatusCode};
use cordis_sdk::Result;
use http_api::{HttpPage, PageAttributes};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(super) struct Pages(Mutex<HashMap<Vec<u8>, PageAttributes>>);

pub(super) struct Page {
    pub key: Vec<u8>,
    pages: Arc<Pages>,
}

impl Drop for Page {
    fn drop(&mut self) {
        self.pages.0.lock().unwrap().remove(&self.key);
    }
}

impl Pages {
    /// ADR-0046 §3.9：记录配置选定的头部及实际对端，8 KiB 预算在升级边界执行。
    pub fn open(
        self: &Arc<Self>,
        headers: &HeaderMap,
        selected: &[String],
        peer: SocketAddr,
    ) -> std::result::Result<Page, StatusCode> {
        let peer = peer.to_string();
        let mut size = peer.len();
        let mut saved = Vec::new();
        for (name, value) in headers {
            if selected
                .iter()
                .any(|selected| selected.eq_ignore_ascii_case(name.as_str()))
            {
                let value =
                    std::str::from_utf8(value.as_bytes()).map_err(|_| StatusCode::BAD_REQUEST)?;
                size += name.as_str().len() + value.len();
                if size > 8192 {
                    return Err(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
                }
                saved.push((name.to_string(), value.to_owned()));
            }
        }
        let key = [
            uuid::Uuid::new_v4().as_bytes().as_slice(),
            uuid::Uuid::new_v4().as_bytes().as_slice(),
        ]
        .concat();
        self.0.lock().unwrap().insert(
            key.clone(),
            PageAttributes {
                headers: saved,
                peer,
            },
        );
        Ok(Page {
            key,
            pages: self.clone(),
        })
    }
}

#[cordis_sdk::interface]
impl HttpPage for Pages {
    async fn attributes(&self, key: Vec<u8>) -> Result<Option<PageAttributes>> {
        Ok(self.0.lock().unwrap().get(&key).cloned())
    }
}
