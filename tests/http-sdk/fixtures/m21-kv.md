# M21 KV 检查点

`m21-kv.state` 来自 M21 三语言迁移验收的最后一次有序退出，原始位置为 `target/test-evidence/phase3-migration-88232-0/phase3-acceptance/phase3-migration-88232-0/case/three-language-migration/state/store.state`。

该记录由 M21 的 TypeScript KV 提供方写出，`kv.snapshot@1` 的 `entries` 值采用整数数组。M22 用同一文件分别恢复 Rust、Python 和 TypeScript 提供方，通过真实 `kv@1.0.get` 比对原值，验证字节串编码更新后的状态兼容。
