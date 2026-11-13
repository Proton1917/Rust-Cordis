//! ADR-0046 §3.11：受信来源区分后端实例与页面所属实例，值由核心建立。

use crate::{CborValue, Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Caller {
    Instance(String),
    PageOf(String),
}

pub fn caller(metadata: &CborValue) -> Result<Option<Caller>> {
    let CborValue::Map(fields) = metadata else {
        return Err(Error::new(2, "视图元数据需要映射"));
    };
    fields
        .iter()
        .find(|(key, _)| key.as_text() == Some("cordis.caller"))
        .map(|(_, value)| cordis_abi::from_value(value.clone()))
        .transpose()
}
