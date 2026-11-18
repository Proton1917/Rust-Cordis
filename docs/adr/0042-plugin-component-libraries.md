# ADR-0042 Rust 组件库与原生入口

可复用的 Rust 组件实现编译为普通库，动态插件入口负责导出实际 ABI 描述。构建工具从 Cargo 报告取得对应动态产物。

同一组件可由示例、测试或正式入口复用。组件类型身份来自相应编译单元，跨库服务依公开接口识别。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0042-plugin-component-libraries.md)
