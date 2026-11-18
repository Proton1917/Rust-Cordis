# ADR-0018 原生代码的执行位置

原生动态库发生进程级错误时，操作系统回收整个进程。核心留在自己的进程中，宿主执行插件代码；两者通过 UP/1 交换消息。

该隔离边界保护核心。宿主可以共享，成员共担进程退出；实例的依赖、效应与正常停用仍各自独立。

[相关规格](../15-universal-socket.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0018-core-isolation.md)
