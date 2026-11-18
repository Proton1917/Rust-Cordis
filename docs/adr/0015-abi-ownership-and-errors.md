# ADR-0015 ABI v1 的错误、缓冲与异步对象

v1 将错误码、分配方缓冲、对象、Future 和 Waker 放入显式的 C 布局。接收方的安全包装负责持有租约，释放时回到原分配方。

每次边界调用的状态与业务错误分开传递。目标 ABI 以固定完成通知承接异步调用，具体语言的 Future 归创建方。

[相关规格](../12-abi-v1.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0015-abi-ownership-and-errors.md)
