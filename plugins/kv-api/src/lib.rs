use cordis_sdk::prelude::*;

/// kv@1：按 UTF-8 键存储字节值，写入相同键会替换原值。
#[cordis_sdk::interface(name = "kv", version = "1.0")]
pub trait Kv: Send + Sync {
    /// 返回当前值；键不存在时返回 None。
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    /// 写入完整值。状态导出冻结期间返回 code = 4。
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;
    /// 按字典顺序返回全部键。
    fn keys(&self) -> Result<Vec<String>>;
    /// 等待先前写入达到实现所声明的持久化程度。
    async fn flush(&self) -> Result<()>;
}

/// kv.snapshot@1 的迁移格式由内存和文件实现共同使用。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub entries: std::collections::BTreeMap<String, Vec<u8>>,
}

/// 每次成功写入后派发的通知。
#[cordis_sdk::event(name = "kv.changed", version = "1.0")]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Changed {
    pub key: String,
}
