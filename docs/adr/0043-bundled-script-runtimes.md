# ADR-0043 脚本解释器的发布布局

独立分发的 Python 或 Node 应保留其发行目录、运行依赖和许可证。复制符号链接时按源目录解析目标，写入包内相对关系。

目标系统的本地扩展参与签名和启动验证。压缩包与桌面应用沿用各自的平台目录结构。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0043-bundled-script-runtimes.md)
