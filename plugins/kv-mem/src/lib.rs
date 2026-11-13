//! DOC-12 §3、ADR-0042：内存 KV 的稳定动态库入口。

pub use kv_mem_component::KvStore;

cordis_sdk::export_plugin!(
    name = "kv",
    version = env!("CARGO_PKG_VERSION"),
    components = [KvStore]
);
