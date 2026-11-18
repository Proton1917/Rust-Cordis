# ADR-0028 内置服务的数据类型

核心、宿主与 SDK 要交换相同的内置接口数据。cordis-types 保存拥有型结构，使用 serde 表达序列化契约。

消费者只引入公共数据类型，内核的运行时对象留在内核中。历史目标为 200 代码行、上限 400 行，最低 Rust 版本为 1.85。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0028-interface-data-types.md)
