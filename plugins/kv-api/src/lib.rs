#[rustfmt::skip]
#[allow(non_snake_case)]
pub mod cordis__kv__kv {
    /**kv.snapshot@1 的迁移格式由内存和文件实现共同使用。
```cordis
default = true
```*/
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Default)]
    pub struct Snapshot {
        #[serde(rename = "entries")]
        pub entries: ::std::collections::BTreeMap<String, Vec<u8>>,
    }
    /**每次成功写入后派发的通知。
```cordis
event = "kv.changed"
kind = "sync"
```*/
    #[cordis_sdk::event(name = "kv.changed", version = "1.0")]
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
    pub struct Changed {
        #[serde(rename = "key")]
        pub key: String,
    }
    /// DOC-15 §7：kv@1.0 的提供方接口及类型化调用代理。
    #[cordis_sdk::interface(name = "kv", version = "1.0")]
    pub trait Kv: Send + Sync {
        ///返回当前值；缺少键时返回 none。
        fn get(&self, key: &str) -> ::cordis_sdk::Result<Option<Vec<u8>>>;
        ///写入完整值。状态导出冻结期间返回 code = 4。
        fn put(&self, key: &str, value: Vec<u8>) -> ::cordis_sdk::Result<()>;
        ///按字典顺序返回全部键。
        fn keys(&self) -> ::cordis_sdk::Result<Vec<String>>;
        ///等待先前写入达到实现所声明的持久化程度。
        async fn flush(&self) -> ::cordis_sdk::Result<()>;
    }
}
pub use cordis__kv__kv::*;
