# ADR-0021 子进程控制与解析器测试

监管器以独立进程组启动插件，通过系统信号、管道和子进程等待完成回收。既有实现使用 nix 与 Tokio。

输入解析的历史模糊测试针对真实边界，原始样本归对应版本。修改无关文档沿用有效证据。

[相关规格](../16-up1-protocol.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0021-process-control-and-fuzzing.md)
