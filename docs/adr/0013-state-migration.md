# ADR-0013 插件状态的继承契约

旧实例通过登记为效应的导出器交出带模式和版本的状态，核心转交不透明数据，新实现负责解释和迁移。

导出缺席和导出失败分别处理。要求持续可靠的业务写入由数据所有者持久保存，快照主要承担内存状态的交接。

[相关规格](../11-plugin-model.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0013-state-migration.md)
