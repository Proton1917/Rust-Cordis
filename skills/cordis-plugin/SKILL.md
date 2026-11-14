---
name: cordis-plugin
description: 为 Rust-cordis 运行时编写、验证和部署插件：Rust 原生插件、Python 与 TypeScript 进程插件、浏览器组件和全栈插件；涉及 cordis.toml、cordis-dev 脚手架、共享接口与 WIT、效应清理、状态继承、运行中替换、HTTP 路由与发布打包时使用。原版 cordis 或 Koishi 的 JavaScript 插件（`import { Context } from 'cordis'`、`ctx.plugin(fn)`）是另一套 API，Rust-cordis 暂不支持，不适用本技能。
---

# Rust-cordis 插件

Rust-cordis 的核心进程不运行插件代码，只管理组件的依赖、加载、效应与替换。Rust 动态库在 cordis-host 中运行；Python 与 Node 插件以独立进程接入；浏览器组件经 http 插件转发的 WebSocket 接入同一个核心。插件与核心之间只走 UP/1 协议。

## 先确认

1. 当前项目是 Rust-cordis 项目：根目录有 `cordis.toml`，插件由 `cordis-dev` 脚手架生成，Rust 插件以 `cordis_sdk::export_plugin!` 导出。
2. 用户的代码若是原版 cordis 写法（从 `cordis` 包导入 `Context`、`ctx.plugin(fn)`、`ctx.on('ready')`），告诉用户 Rust-cordis 尚不能运行原版插件，需要改写为本技能的 SDK 写法。原版兼容的设计在 Rust-cordis 仓库 `docs/17-upstream-ecosystem-compat.md`，尚未实现。
3. 用 `cordis version` 查看核心版本，`cordis-dev version` 查看开发工具。沿用项目已有的语言和接口。

## 选择入口

新项目用 `cordis new demo`。插件名以小写字母开头，只含小写字母、数字和短横线。

| 命令 | 组件形态 | 按需阅读 |
|---|---|---|
| `cordis new-plugin hello --rust` | Rust 原生插件 | [Rust 接口](references/rust-api.md)、[资源与状态](references/resources-state.md) |
| `cordis new-plugin hello --python` | Python 进程插件 | [Python 插件](references/python.md) |
| `cordis new-plugin hello --ts` | Node 中的 TypeScript 插件 | [TypeScript 插件](references/typescript.md) |
| `cordis new-plugin hello --web` | 浏览器组件 | [网页与全栈插件](references/web.md) |
| `cordis new-plugin hello --rust --web` | 后端加浏览器组件；后端也可用 `--python`、`--ts` | 同上 |
| `cordis new-plugin api --rust --http` | HTTP 处理方；也可用 `--python`、`--ts` | [HTTP、流与身份](references/http.md) |

版本要求：Rust 插件与接口库 Rust 1.85 及以上；Python SDK 3.11 及以上；TypeScript 进程 SDK Node 22 及以上。

方法名来自配置或外部输入时，读[按名称调用](references/dynamic.md)。运行、验证与替换读[验证与部署](references/verification-deployment.md)，发布读[打包与独立运行时](references/packaging.md)。

## 每个插件都遵守的规则

1. **声明与实际一致。** 组件的必需与可选依赖写在 inject，提供的服务写在 provides，监听的事件写在 listens，迁移的状态写模式名与版本范围。未声明的接口取不到。
2. **resolve 只检查配置，apply 负责激活。** 必需依赖缺席时核心不调用 apply，实例保持 Pending；依赖恢复后自动加载。
3. **资源归属当前实例。** 文件、连接、监听器在效应中取得，清理登记在同一效应。后台工作用 SDK 的任务入口（Rust `ctx.spawn`/`ctx.spawn_thread`，Python 与 TypeScript `context.task`），不要用脱离实例的线程、全局 `tokio::spawn`、裸 `asyncio.create_task` 或 `setInterval`，否则替换后旧代码继续运行。
4. **业务状态经导出器迁移。** 写入时在同一临界区检查冻结标志再修改；导出器读同一份状态。
5. **调用服务或派发事件前释放本地锁**，让重入的回调能继续执行。
6. **跨进程事件只是通知。** 需要返回值的交互用服务方法。
7. **替换保持兼容。** 逻辑插件名、组件名、接口主版本不变；追加方法提升接口次版本，追加在末尾。
8. **公开接口用 WIT 或共享 Rust 接口 crate 管理。** 三种语言的绑定由 `cordis idl gen` 从同一份 WIT 生成。

## 常见故障

| 现象 | 常见原因 | 处理 |
|---|---|---|
| 实例一直 Pending | 必需依赖没有提供方，或 inject 写的接口名、主版本与提供方 provides 不一致 | `cordis status` 查看实例缺少的接口；核对两边的接口字符串 |
| 取服务时报未声明依赖 | 取用的接口不在 inject 中 | 补声明；Rust 动态名用 `Inject::with_name` |
| 写入返回错误码 4（导出期间冻结） | 替换或检查点正在导出状态 | 请求式写入把错误交给调用方；后台循环等下一轮再写 |
| 替换后旧逻辑仍在执行 | 任务或线程没有归属实例 | 改用规则 3 的任务入口 |
| 一个原生插件 panic，整个宿主分组退出 | Cargo profile 设置了 `panic = "abort"` | 使用 `panic = "unwind"` |
| 替换后状态丢失或回滚 | 未声明状态模式，或继承时模式名、版本核对失败 | 核对 state 声明与 apply 中的继承检查 |
| 事件回调或服务调用卡住 | 持有本地锁时调用服务或派发事件 | 规则 5 |
| Python 插件 `ModuleNotFoundError` | 依赖没有装进插件的 `.dependencies` | 按 [Python 插件](references/python.md) 安装 |
| `cordis pack` 报告私有路径 | 产物含本机 HOME 或用户名 | 见 [打包](references/packaging.md) |

## 交付记录

说明使用的 SDK 版本、构建与运行命令、实际观察到的行为（服务调用、清理、状态继承、替换结果）以及验证覆盖的范围。

在 Rust-cordis 仓库内修改 SDK、模板或官方插件时，另读[维护仓库本身](references/repository.md)。
