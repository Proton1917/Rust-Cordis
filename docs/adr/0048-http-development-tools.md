# ADR-0048 HTTP 工程模板与密钥交付

开发工具为 Rust、Python 和 TypeScript 生成能实际访问的 HTTP 处理方。应用配置引用密钥名，运行环境中的 secrets 服务负责读取它的值。

构建和打包只交付声明的公开配置及资源。凭据文件由部署者在目标环境准备，应用按调用来源取得获准的密钥。

[相关规格](../13-runtime-architecture.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0048-http-development-tools.md)
