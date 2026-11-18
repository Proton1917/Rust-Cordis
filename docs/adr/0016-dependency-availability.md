# ADR-0016 依赖可用性与定义退役

可选依赖缺席时实例仍可工作；必需依赖缺席或次版本不足时进入等待。内核按绑定和谓词计算结果，谓词在锁外执行。

已被替换的定义在活动引用归零后可退出登记。重新提供依赖会形成新纪元，调用方从新绑定取得对象。

[相关规格](../11-plugin-model.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0016-dependency-availability.md)
