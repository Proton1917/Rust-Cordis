# ADR-0055 原生插件的 Tokio 执行上下文

动态库有独立编译的 Tokio 及线程局部状态。主分支 SDK 的 tokio 特性在首次进入库时创建一份单工作线程运行时，提供调用第三方网络库所需的上下文。

库退役时关闭该运行时。新部署设计将执行资源的成本区分到宿主与库；纯计算模板可关闭该特性，依赖 Tokio 的插件显式采用相应模板。

[相关规格](../11-plugin-model.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0055-plugin-tokio-context.md)
