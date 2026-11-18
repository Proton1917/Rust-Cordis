# ADR-0025 隔离域、拦截与子组件

部署或插件可派生上下文，设置服务命名域并叠加拦截元数据。子组件作为父实例的效应挂载。

核心按调用方实际视图解析服务；声明 scoped 的提供方可以取得与该调用上下文绑定的服务对象。

[相关规格](../16-up1-protocol.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0025-cross-process-composition.md)
