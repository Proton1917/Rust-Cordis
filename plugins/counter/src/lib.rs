//! DOC-12 §3、ADR-0042：计数组件的稳定动态库入口。

pub use counter_component::{Counter, CounterConfig};

cordis_sdk::export_plugin!(
    name = "counter",
    version = env!("CARGO_PKG_VERSION"),
    components = [Counter]
);
