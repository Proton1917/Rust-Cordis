# ADR-0051 同步服务的宿主线程交接

同步插件如果占住所有执行线程等待异步工作，其他调用会等到截止时间。ABI 1.1 增加宿主协同等待入口，由宿主安排线程交接。

现行 Rust SDK 可在同步上下文使用 Context::wait，在异步上下文直接 await。新版固定调用入口应保留可推进的控制通信与明确的完成交付。

[相关规格](../12-abi-v1.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0051-native-synchronous-wait.md)
