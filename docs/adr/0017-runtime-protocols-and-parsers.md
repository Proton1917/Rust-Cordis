# ADR-0017 部署格式与运行时依赖

部署配置、版本号、二进制消息和命令行参数各使用对应的成熟解析库。依赖随解析这一数据的模块声明。

主程序读取 TOML，发布资料与状态使用结构化输出。内核保持与应用协议独立，工具和测试依赖各归其构建目标。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0017-runtime-protocols-and-parsers.md)
