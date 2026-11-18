# ADR-0024 核心之外的工具与应用功能

核心提供运行、状态和服务管理。工程生成、接口生成和发布归 cordis-dev；文件观察归 watch；桌面窗口由独立插件提供。

管理操作经显式授权进入同一运行时入口。业务应用依赖公开接口与工具输出，内部磁盘格式由其所有者维护。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0024-everything-else-is-a-plugin.md)
