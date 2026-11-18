# WIT 多语言绑定夹具

[types.wit](types.wit) 和仓库的 [kv.wit](../../../../../plugins/kv-api/kv.wit) 是接口生成器的输入。前者用于复合类型、资源、借用与异步值，后者用于 KV 方法、状态和事件。Rust 夹具通过 `CORDIS_IDL_GENERATED` 读取本次生成的代码。Python 和 TypeScript 端使用对应 SDK 交换 CBOR 值。

需要检查生成器或跨语言类型映射时，从仓库根目录可运行 `cargo test -p cordis-dev --test idl -- --nocapture`。调用方使用 Cargo、Conda base、Node 和 npm，将生成源码、数据和日志放入 `target/idl-acceptance` 的独立目录。依赖安装目录与包管理缓存由夹具设置。[调用入口](../../idl.rs)
