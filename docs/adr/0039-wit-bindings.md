# ADR-0039 WIT 数据与资源的语言映射

开发工具用 wit-parser 生成公共接口模型，再生成各语言的类型与调用代理。复合值、资源和异步返回沿同一套传输定义解释。

list<u8> 在任何嵌套位置都用 CBOR 字节串表示；资源由调用期容器转交所有权。历史 ABI 的方法顺序进入槽位，新设计改用稳定方法身份。

[相关规格](../12-abi-v1.md) · [历史版本原文](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/adr/0039-wit-bindings.md)

## 语言绑定速查

同一份接口的资源身份和整数宽度在各语言中有明确表示。生成器应使用统一模型解释以下类型。

| WIT 类型 | Rust | Python | TypeScript | 传输表示 |
|---|---|---|---|---|
| 整数、浮点、bool、char、string | 对应标量，char 为 Unicode 标量 | int、float、bool、str | 64 位整数为 bigint，其余 number、boolean、string | 对应 CBOR 标量 |
| record | 具名 struct | dataclass | interface | 字段名映射 |
| variant / enum | enum | 具名变体 / Enum | 判别联合 / 字符串联合 | variant 为单键映射，空载荷为 null；enum 为字符串 |
| list、tuple、固定长度 list | Vec、tuple、固定长度容器 | list、tuple | Array、tuple | 数组；方法位置的 list<u8> 使用字节串 |
| map | BTreeMap | dict | Map | CBOR 映射，重复键遵循 WIT 的最后值规则 |
| flags | 带布尔字段的值 | 带布尔字段的值 | 带布尔字段的值 | 字段名到布尔值的映射 |
| option | Option | 可选值及必要的 Some 包装 | 可选值及必要的 Some 包装 | None 为 null；Some 的载荷为 null 或既有 Some 标签时，用 tag 60001 包裹 |
| result<T,E> 方法结果 | cordis_sdk::Result<T> | T 或 CordisError | Promise<T> 或 CordisError | 成功值与统一错误通道；E 进入方法描述，业务细节由 detail 承载 |
| 数据中的 result | 带 ok/err 分支的数据 | 带 ok/err 分支的数据 | 带 ok/err 分支的数据 | 单键映射 |
| resource / own | Service<dyn I> | 有类型的服务代理 | 有类型的服务代理 | tag 60000 的对象编号与既有引用管理 |
| borrow | 调用期资源借用 | 调用期借用代理 | 调用期借用代理 | 调用期保有独立引用，调用方原句柄保留 |
