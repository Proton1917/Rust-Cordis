# ADR-0037 桌面窗口的主线程

系统 WebView 和菜单需要平台 UI 主线程。窗口作为独立进程插件运行，以系统事件循环创建窗口，所需服务仍由核心解析。

窗口拥有平台句柄和页面加载，下线时释放相应资源。macOS 的实际启动、退出与桌面行为据当时窗口场景评价。

[相关规格](../15-universal-socket.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0037-window-process.md)
