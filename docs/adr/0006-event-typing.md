# ADR-0006 类型化事件与派发方式

每项事件声明参数、结果和派发族。监听器通过上下文登记成效应，事件身份及回调类型由定义方确定。

异步派发、顺序派发、瀑布等行为分别按接口处理参数和结果；释放实例时撤回监听器。

[相关规格](../01-semantics.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0006-event-typing.md)
