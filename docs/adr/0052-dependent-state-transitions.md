# ADR-0052 依赖方状态与调用退役

替换不仅影响显式目标。监管器依据实际服务绑定取得需要重新执行 apply 的依赖方，统一导出并交接这些实例的状态。

回滚报告应说明确认窗口内撤销的实例及时间。正在使用过期提供方的调用和流收到退役结果，已提交的业务写入由业务所有者持久保存。

[相关规格](../11-plugin-model.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0052-dependent-state-transitions.md)
