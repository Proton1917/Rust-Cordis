use crate::Value;
use serde::{Deserialize, Serialize};

/// 两个方向分别维护请求编号，响应指向接收方先前发出的请求。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Request {
        id: u64,
        method: String,
        #[serde(deserialize_with = "crate::codec::params")]
        params: Value,
    },
    Response {
        id: u64,
        #[serde(with = "outcome")]
        result: Result<Value, WireError>,
    },
    Notification {
        method: String,
        #[serde(deserialize_with = "crate::codec::params")]
        params: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{message} (code {code})")]
#[serde(deny_unknown_fields)]
pub struct WireError {
    pub code: u32,
    pub message: String,
    #[serde(deserialize_with = "crate::codec::detail")]
    pub detail: Value,
}

impl WireError {
    pub fn new(code: u32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: Value::Null,
        }
    }
}

mod outcome {
    use super::*;
    use serde::ser::SerializeMap;

    pub fn serialize<S: serde::Serializer>(
        value: &Result<Value, WireError>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        match value {
            Ok(value) => map.serialize_entry("ok", value)?,
            Err(error) => map.serialize_entry("error", error)?,
        }
        map.end()
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Result<Value, WireError>, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Outcome {
            Ok(#[serde(deserialize_with = "crate::codec::result")] Value),
            Error(WireError),
        }
        Ok(match Outcome::deserialize(deserializer)? {
            Outcome::Ok(value) => Ok(value),
            Outcome::Error(error) => Err(error),
        })
    }
}
