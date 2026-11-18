# ADR-0022 通用 ABI 的代码预算

为对象补充按名调用、CBOR 资源转交和释放语义后，ABI 的缓冲、对象与异步值仍依赖同一套所有权规则。历史决定将这些实现留在同一 crate。

当时记录产品源码 2353 行，目标 2500 行，上限 3000 行。该数字描述旧实现的预算；新接口的要求从调用与释放行为确定。

[相关规格](../12-abi-v1.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0022-generic-abi-budget.md)
