# ADR-0053 原生库副本的磁盘寿命

宿主装入库时持有文件副本，监管器的重启计划也会引用该副本。复制、装载与重启期间通过文件租约保护实际来源。

引用释放后的回收按副本独立执行。映像驻留属于卸载策略，磁盘副本属于文件所有权；处理删除失败时保留有效运行状态和诊断。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0053-native-cache-ownership.md)
