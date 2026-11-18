# ADR-0032 工程模板与终端日志

开发工具随版本交付适配 SDK 的插件模板，生成真实可构建项目。已有空目录可作为目标；包含用户文件的冲突由工具在写入前报告。

终端日志由 console-log 插件格式化，标签、时间、多行正文和等级属于输出端。UP/1 的标准输出继续传送协议帧。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0032-console-format-and-scaffold.md)
