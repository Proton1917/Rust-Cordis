# ADR-0038 HTTP 页面入口与静态资源

HTTP 插件拥有监听端口、文件映射、会话属性及网页入口。核心管理页面连接和实例；端口、地址和静态资源映射属于 HTTP 部署配置。

窗口从这一入口打开页面。历史实现使用环回地址与每次激活产生的令牌；对外路由与授权在后续 HTTP 契约中扩展。

[相关规格](../15-universal-socket.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0038-http-entry-service.md)
