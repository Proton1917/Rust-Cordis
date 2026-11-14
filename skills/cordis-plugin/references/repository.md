# 维护 Rust-cordis 仓库本身

本文件只在 Rust-cordis 仓库内工作时适用：修改 SDK、脚手架模板或官方插件。在用户项目中编写插件时不需要阅读，文中的路径在用户项目里不存在。

## 模板来源

`cordis new` 与 `cordis new-plugin` 生成的文件来自 cordis-dev 构建时嵌入的模板。修改模板后重新构建 cordis-dev。

| 内容 | 生成位置 | 仓库维护源 |
|---|---|---|
| Rust 插件 | `cordis new-plugin hello --rust` | `skills/cordis-plugin/templates/plugin.rs`、`plugin.toml`（`crates/cordis-dev/src/scaffold.rs` 以 `include_str!` 读取） |
| Rust 接口、提供方、契约 | 用户手动按 rust-api.md 的布局创建 | `skills/cordis-plugin/templates/interface.*`、`provider.*`、`contract.*` |
| HTTP 处理方 | `--http` | `skills/cordis-plugin/templates/http.rs`、`http.py`、`http.ts` |
| Python 插件 | `plugins/hello/main.py` 等 | `crates/cordis-dev/templates/python/main.py`；SDK 来自 `sdks/python/cordis_sdk`；清单由 `scaffold/process.rs` 生成 |
| TypeScript 插件 | `plugins/hello/src` 等 | `crates/cordis-dev/templates/typescript`；SDK 来自 `sdks/typescript/cordis-sdk/src` |
| 网页部分 | `plugins/hello/web` | `crates/cordis-dev/templates/web`；`scaffold/web.rs` 写入组件名与清单 |
| 原生全栈构建脚本 | `plugins/hello/build.sh` | `crates/cordis-dev/templates/web/build.sh` |
| WIT 绑定 | `idl gen --output` 指定的路径 | `crates/cordis-idl` 的解析与各语言生成器 |

`crates/cordis-runtime/tests/skill_examples.rs` 用当前 SDK 编译 `templates/` 下的 Rust 模板（DOC-14 `SK-3`）。修改模板或 SDK 后运行该测试。

## TypeScript SDK 与网页宿主的构建

在仓库根目录执行：

```sh
cd sdks/typescript
npm ci --ignore-scripts --no-audit --no-fund
npm run build
cd ../..
cargo build -p cordis-http
```

该入口生成 `cordis-sdk/dist`，同时更新 `cordis-web/dist/cordis-web.js` 与 `plugins/http/assets/cordis-web.js`；HTTP 动态库在随后的 Cargo 构建中嵌入更新后的脚本。SDK 独立构建入口为 `cordis-sdk/build.mjs`，workspace 与 cordis-web 的 npm build 复用同一编译函数。

## Python 依赖的仓库约定

仓库测试经 `cordis_testkit::python::install_dependencies` 安装插件依赖，解释器、目标目录与缓存规则与 python.md 中的命令相同。本机 Python 一律经 `conda run -n base python` 调用。

## 发布路径映射

`tools/path-remapping.json` 定义路径映射，`tools/build_paths.py` 生成与 `cordis pack` 相同的 `CARGO_ENCODED_RUSTFLAGS`，供预构建核心与宿主使用。机制见 ADR-0049，命令见 guide 的“构建路径隐私”。

## 仓库示例

| 主题 | 位置 |
|---|---|
| 共享 KV 接口 `kv@1` | `plugins/kv-api`：`get(&str) -> Result<Option<Vec<u8>>>`、`put(&str, Vec<u8>)`、`keys() -> Result<Vec<String>>`（字典序）、`async flush()`；事件 `kv.changed@1.0`；状态 `kv.snapshot` 版本 1，三种语言共用 |
| KV 提供方 | Rust `plugins/kv-mem`、`kv-file`；Python `plugins/kv-py`；TypeScript `plugins/kv-ts` |
| KV 契约套件 | `plugins/kv-contract` |
| HTTP 处理方 | `plugins/http-echo`、`http-echo-py`、`http-echo-ts`（回显与 `/file` 上传下载） |
| 网页与全栈 | `examples/web`（纯网页笔记、Rust KV 全栈、Python KV 全栈）；页面域与子组件见 `tests/web-sdk/fixtures/realm-counter` |
