#[rustfmt::skip]
#[allow(non_snake_case)]
pub mod test__payload__exchange {
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
    pub struct Part {
        #[serde(rename = "data")]
        pub data: ::cordis_sdk::Bytes,
    }
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
    pub enum Selection {
        #[serde(rename = "data")]
        Data(::cordis_sdk::Bytes),
        #[serde(rename = "empty")]
        Empty(()),
    }
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
    pub struct Bundle {
        #[serde(rename = "record-value")]
        pub record_value: Part,
        #[serde(rename = "variant-value")]
        pub variant_value: Selection,
        #[serde(rename = "map-value")]
        pub map_value: ::std::collections::BTreeMap<String, ::cordis_sdk::Bytes>,
        #[serde(rename = "option-value")]
        pub option_value: Option<::cordis_sdk::Bytes>,
        #[serde(rename = "result-value")]
        #[serde(with = "__wire_5")]
        pub result_value: ::std::result::Result<::cordis_sdk::Bytes, String>,
    }
    /// DOC-15 §7：test.byte-payload@1.0 的提供方接口及类型化调用代理。
    #[cordis_sdk::interface(name = "test.byte-payload", version = "1.0")]
    pub trait Exchange: Send + Sync {
        async fn exchange(&self, value: Bundle) -> ::cordis_sdk::Result<Bundle>;
    }
    mod __wire_5 {
        #[allow(unused_imports)]
        use super::*;
        use serde::{Serialize, Deserialize};
        use ::cordis_sdk::__private::abi;
        type __Value = ::std::result::Result<::cordis_sdk::Bytes, String>;
        fn to_wire(value: &__Value) -> ::std::result::Result<abi::CborValue, String> {
            match value {
                Ok(item_0) => {
                    Ok::<
                        _,
                        String,
                    >(
                        abi::CborValue::Map(
                            vec![
                                (abi::CborValue::Text("ok".into()), (Ok::< _, String >
                                (abi::CborValue::Bytes(item_0.to_vec()))) ?)
                            ],
                        ),
                    )
                }
                Err(item_0) => {
                    Ok::<
                        _,
                        String,
                    >(
                        abi::CborValue::Map(
                            vec![
                                (abi::CborValue::Text("err".into()),
                                (abi::CborValue::serialized(item_0).map_err(| error | error
                                .to_string())) ?)
                            ],
                        ),
                    )
                }
            }
        }
        fn from_wire(value: abi::CborValue) -> ::std::result::Result<__Value, String> {
            match value {
                abi::CborValue::Map(mut items) if items.len() == 1 => {
                    let (key, item_0) = items.pop().expect("已检查 result 长度");
                    match key.as_text() {
                        Some("ok") => {
                            Ok::<
                                _,
                                String,
                            >(
                                Ok(
                                    (item_0
                                        .deserialized::<::cordis_sdk::Bytes>()
                                        .map_err(|error| error.to_string()))?,
                                ),
                            )
                        }
                        Some("err") => {
                            Ok::<
                                _,
                                String,
                            >(
                                Err(
                                    (item_0.deserialized().map_err(|error| error.to_string()))?,
                                ),
                            )
                        }
                        _ => Err("result 分支需要 ok 或 err".into()),
                    }
                }
                _ => Err("result 需要单键映射".into()),
            }
        }
        pub fn serialize<S: serde::Serializer>(
            value: &__Value,
            serializer: S,
        ) -> ::std::result::Result<S::Ok, S::Error> {
            let value = to_wire(value).map_err(serde::ser::Error::custom)?;
            value.serialize(serializer)
        }
        pub fn deserialize<'de, D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> ::std::result::Result<__Value, D::Error> {
            let value = abi::CborValue::deserialize(deserializer)?;
            from_wire(value).map_err(serde::de::Error::custom)
        }
    }
}
pub use test__payload__exchange::*;
