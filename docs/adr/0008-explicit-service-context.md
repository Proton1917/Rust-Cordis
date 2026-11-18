# ADR-0008 服务定义点与调用方上下文

同一服务可由多个实例取得。提供方掌握实现，调用方的上下文决定本次登记的效应以及生效的拦截元数据。

Rust 侧用 Scoped<K> 显式携带使用点。替换提供方后由内核重新绑定，调用者继续依其声明使用服务。

[相关规格](../01-semantics.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0008-explicit-service-context.md)
