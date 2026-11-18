# ADR-0012 原生库退役与映像回收

已停止调用的旧库仍可能留在进程地址空间。v1 用库租约覆盖代码和对象的使用期；默认 unload = "never"，也提供 when-unleased 以在租约归零后关闭句柄。

旧映像数量超过配置阈值时执行有状态的组重启，默认阈值为 20。重启影响整个宿主，常规实例替换按真实依赖确定范围。

[相关规格](../15-universal-socket.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0012-unload-semantics.md)
