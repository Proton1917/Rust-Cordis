# ADR-0040 脚本插件的协议实现

Python 使用 cbor2，TypeScript 使用 cbor-x 编解码 UP/1。脚手架同时提供插件入口、依赖声明及接入核心的 SDK。

各语言在自己的事件循环运行。生成绑定遵循同一组调用、错误、取消和资源归属规则，打包时包含实际需要的解释器和运行依赖。

[相关规格](../16-up1-protocol.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0040-script-sdk.md)
