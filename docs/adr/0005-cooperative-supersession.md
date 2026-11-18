# ADR-0005 加载被取代时的清理顺序

一次正在执行的加载可能已经建立资源。新目标更新实例纪元，旧加载在效应步骤边界观察变化，收束后释放已经登记的内容。

清理按已有逆操作推进，随后才开始新的迁移。Future 的执行和资源的实际释放有明确的所有者。

[相关规格](../02-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0005-cooperative-supersession.md)
