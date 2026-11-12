# ADR-0020 以 WIT 为接口描述语言

| 项 | 内容 |
|---|---|
| 状态 | 已接受（2026-11-12） |
| 影响 | `DOC-11` §3；`DOC-13` §3.3；`DOC-15` §7、`M17` |

## 问题

第二阶段的接口写成 Rust trait（`#[interface]`）。Python 与 TypeScript 插件的作者需要同一份接口定义，且不应为此读写 Rust。

## 选项

| 选项 | 内容 | 代价 |
|---|---|---|
| A | 以 Rust trait 为来源，导出模式供其他语言使用 | 其他语言的作者定义新接口时仍要写 Rust |
| B | 自定义一种接口格式 | 需要自行维护语法、解析器与文档 |
| C | WIT（WebAssembly Interface Type） | 需要编写 WIT 到 Rust、Python、TypeScript 的代码生成器 |

## 判据

1. 各语言的作者只读写一份中立的接口定义。
2. 格式有独立于本项目的公开规范。
3. 能表达异步方法、记录、变体、可选值、结果与资源。
4. 今后接入 WebAssembly 插口时不需要第二套接口定义。

## 决定

选 C。依据：WIT 是 WebAssembly 组件模型的标准接口语言；WASI 0.3 已发布，WIT 支持 `async func`、`future<T>`、`stream<T>`；Rust 解析库 `wit-parser` 可用。A 不满足判据 1，B 不满足判据 2、4。

- 接口版本取 WIT 包版本，主版本与次版本对应 `DOC-11` §3，修订号不参与匹配。
- `cordis idl gen --lang rust|python|ts` 由 WIT 生成接口、数据类型与调用方代理。WIT 类型到各语言及 CBOR 的映射见 `DOC-15` §7。
- `wit-parser` 只在 `cordis idl` 子命令中使用，不进入核心的运行时路径。

## 后果

- `M17` 起 `kv-api` 改为由 `kv.wit` 生成，生成结果与第二阶段手写的 `#[interface]` 代码行为一致。
- 第二阶段的 `#[interface]` 宏继续作为生成代码的展开目标。
