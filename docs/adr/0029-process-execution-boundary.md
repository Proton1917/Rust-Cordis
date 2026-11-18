# ADR-0029 核心与开发工具的进程操作

插件运行与构建工具均需启动程序、等待完成并清理整组进程。cordis-process 统一 Peer、进程组寿命和输出捕获。

运行时负责长期服务，开发工具负责短期构建和验收程序；两者各持自己的进程所有权。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0029-process-execution-boundary.md)
