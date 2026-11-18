# ADR-0044 部署目标与稳定状态

纯代码替换要求受影响的运行服务恢复可用。配置操作主动撤去服务时，其消费者清理资源并进入 Pending，就是正确的稳定结果。

历史场景中误用版本替换的 Active 判据会把刚移除的提供方恢复。两类操作共用快照与事务实现，通过目标状态决定提交结果。

[相关规格](../11-plugin-model.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0044-configuration-stability.md)
