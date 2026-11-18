# ADR-0014 宿主、业务接口与状态的版本

ABI 主次版本描述机器边界；接口主次版本描述方法和数据；状态模式及版本由业务插件命名。调用方在这三个位置判断不同的兼容条件。

既有约定要求 ABI 后继主版本发布后，旧主版本至少再支持三个核心年度版本。业务接口可并存多个主版本，以适配插件承接迁移。

[相关规格](../10-evolution-strategy.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0014-versioning.md)
