# M23 动态调用验收

动态调用通过 SDK 进入既有 V-2 通用入口。核心产品 src、Cargo.toml 与 cordis-abi 相对 review 提交 70db380 保持一致；实现位于 Rust SDK、TypeScript SDK 和开发工具。Python 复用已有 Remote.invoke。

| 编号 | 实际场景与结果 | 原始记录 |
|---|---|---|
| AU-27 | 同一个 Rust 使用方对同组原生、跨组原生、Python、TypeScript 的 KV 执行 get/put/keys/flush；类型化与动态结果相等，缺席键一致 | [四种提供方](m23-matrix.json) |
| AU-27a | 真实 1 MiB 文件流：原始 invoke 返回参数错误后资源已释放；invoke_as_async 取得 Service，逐字节读完并释放；伪造资源标签返回参数错误 | [四种提供方](m23-matrix.json) |
| AU-28 | 已存在但未声明的普通服务和 secrets 都返回 code 4，与类型化一致；未知方法 code 3；参数数量、类型及裸资源标签 code 2；核心内置 Runtime 沿原规则取得成功 | [四种提供方](m23-matrix.json) |
| AU-29 | 两种调用持续读取期间，内存提供方替换为文件提供方并产生文件；失败候选回滚后继续读取；提供方卸下后使用方 Pending | [生命周期](m23-lifecycle.json) |
| AU-30 | Scoped 提供方收到相同 cordis.caller，值为实际使用方实例 probe | [四种提供方](m23-matrix.json) |
| 脚本入口 | Python Remote.invoke 与 TypeScript getDynamic 实际读写字节值、处理未知方法并拒绝裸资源编号 | [脚本调用](m23-scripts.json) |
| 脚手架版本 | 生成项目包含 SDK 0.2.0 约束和开发工具构建时的源码提交；本机 SDK 项目完成构建与运行 | workspace 的 scaffold 目标 |

本轮 `cargo test --workspace --no-fail-fast` 共 339 项通过、1 项失败，独立的 RM-11 测量入口保持忽略。失败项 readme_first_plugin_builds_from_pinned_git_sdk 引用构建时的 70db380，而该提交的 SDK 版本为 0.1.0。设计方 72c36897 明确将这一条安排在 M23 推送后，以 `CORDIS_SOURCE_REVISION=<M23 提交>` 复验；结果按实际日志继续记录。其余完整用例已通过。

fmt、workspace/all-targets clippy、177 条上游矩阵通过；工作区中的 unsafe 审计和 ABI 布局检查通过。ABI 与宏的实现保持，Miri 复用 M22 的 13 条边界与布局结果。编译目录继续采用 target/debug 与 target/shared，成功现场由 TestGuard 归档，日志入口为 target/dynamic-m23/evidence。

发行版本计划为 v0.2.0。Rust SDK、cordis-dev 与 TypeScript SDK 为 0.2.0；核心、宿主、Python SDK 和官方插件保持 0.1.0，ABI v1.0 与 UP/1 保持。夹具 Cargo.lock 中 SDK 项的版本联动是 SDK 升到 0.2.0 的机械结果，其余依赖保持。
