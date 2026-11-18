# ADR-0007 实例视图与句柄寿命

派生上下文可能比组件本次激活存活更久。实例节点保存身份，活动记录持有节点；释放迁移结束时移走回调与配置，在状态锁外交还。

最后一个节点句柄释放后，已失活记录从内部存储移除。仍持有上下文的调用方可以识别其来源。

[相关规格](../02-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0007-fiber-scope-lifetime.md)
