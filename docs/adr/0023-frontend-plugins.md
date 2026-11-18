# ADR-0023 浏览器页面的接入

网页模块在浏览器执行，HTTP 插件终结 WebSocket，再把页面连接转交给核心。每个页面独立取得实例和上下文域。

页面关闭后撤回该连接所属资源。后端服务更新时，页面实例遵循同一依赖语义。

[相关规格](../15-universal-socket.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0023-frontend-plugins.md)
