# ADR-0027 子进程退出的观察与回收

进程组信号以插件组为对象，waitid 观察相应 Child 的退出状态，再由唯一所有者执行回收。SIGCHLD 用于唤醒等待者。

观察和收割分开，可防止提前回收造成状态缺失，也能让核心和开发工具使用同一套安全进程接口。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0027-process-group-reaping.md)
