# ADR-0004 服务的类型与名称

进程内服务键以关联类型确定值的 Rust 类型，以公开名称标识接口。部署配置和远端插件通过名称描述能力。

同名提供方随上下文解析。加载器持有独立名称，插件代码卸载之后仍可识别旧绑定和错误来源。

[相关规格](../01-semantics.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0004-typed-keys.md)
