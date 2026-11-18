# ADR-0033 部署分组与上下文继承

配置中的 scope 组织父子关系、禁用状态、隔离域和拦截元数据；group 选择原生宿主。两个插件可以共享一个宿主却拥有不同的服务视图。

运行时展开逻辑组，检查父关系并生成实例目标。隔离映射和拦截配置逐层继承，管理授权来自基础实例配置。

[相关规格](../15-universal-socket.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0033-configuration-scopes.md)
