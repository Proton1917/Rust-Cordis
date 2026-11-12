//! DOC-15 §6.4、ADR-0025：部署配置与 UP/1 请求共用的隔离域和拦截参数。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Isolation {
    pub interface: String,
    pub realm: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Interception<V> {
    pub interface: String,
    pub meta: V,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextOptions<V> {
    pub isolate: Vec<Isolation>,
    pub intercept: Vec<Interception<V>>,
}

impl<V> Default for ContextOptions<V> {
    fn default() -> Self {
        Self {
            isolate: Vec::new(),
            intercept: Vec::new(),
        }
    }
}
