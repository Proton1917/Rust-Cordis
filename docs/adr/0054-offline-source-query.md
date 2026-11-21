# ADR-0054 停机查询已确认的程序

应用部署工具需要知道每个实例最后确认使用的版本。cordis status --offline 读取运行时提交的程序来源，输出 schema_version 和逐实例映射。

首版字段为 declared_source、installed_reference 和 plugin_directory。确认中的候选沿用当前已提交指向，记录读取失败由 CLI 返回错误；应用据公开数据管理版本留存。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0054-offline-source-query.md)

## 输出字段

| 字段 | 类型与语义 |
|---|---|
| `schema_version` | 整数，首版固定为 1 |
| `instances` | 以部署实例名为键的对象，包含持久记录中的全部实例；序列化按实例名排序 |
| `declared_source` | 字符串，该绑定确认时记录的基础程序声明；管理创建的实例沿用运行时记录的声明规则 |
| `installed_reference` | 字符串，该实例最后持久确认的程序引用；消费者按不透明引用处理 |
| `plugin_directory` | 字符串，该绑定对应的插件安装根目录，输出为绝对路径 |

查询取得一次完整提交的视图。有效空记录返回空映射；读取、解析和权限故障通过标准错误及非零退出码报告。

v0.2.10 在状态目录内使用跨进程读写锁。提交和恢复文件时，写方持有独占锁。离线查询持有共享锁完成来源选择与读取。核心中途退出后，查询从仍在生效的回退记录读取先前提交的来源。
