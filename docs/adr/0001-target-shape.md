# ADR-0001 论文模型与参考实现

论文定义组件的时间与空间组合，上游 TypeScript 源码给出具体的服务、事件和清理行为。Rust 移植以可观察结果为验收依据。

运行时可以使用类型化句柄、Future 和不同的模块布局。参考实现中依赖 JS 对象模型的行为，在一致性矩阵列明对应表达或适用范围。

[相关规格](../00-strategy.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0001-target-shape.md)
