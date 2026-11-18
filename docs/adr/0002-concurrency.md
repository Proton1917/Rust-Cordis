# ADR-0002 内核状态与可重入调用

多个线程可能同时访问同一实例。内核用一把状态锁确定事件快照、依赖变化与待执行动作，由串行调和循环推进目标状态。

调用用户函数、轮询 Future 和唤醒等待者放在锁外。监听器可以在派发期间撤回登记，组件也可以在加载期间改变目标。

[相关规格](../02-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0002-concurrency.md)
