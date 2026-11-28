---
name: cordis-first-principles
description: Rust-cordis 的第一性原理与插件构建：以“极简通用内核 + 一切皆插件 + 时空可组合性（时间可撤销效应与空间反应式依赖）”为原点，再用唯一事实所有者与四条推论把能力放进正确的插件，不改核心；用 Rust、Python、TypeScript 或网页 SDK 编写插件，处理依赖、效应清理、状态继承、运行中替换、原生 ABI 与宿主内存控制、认证与机密、长任务与超时、接口升级、HTTP 路由、发布打包和生产部署。在 Rust-cordis 上设计新功能、把现有应用迁到 Rust-cordis、拆分服务、判断某项能力该放在哪里、编写或审查插件时使用。原版 cordis 或 Koishi 的 JavaScript 插件（`import { Context } from 'cordis'`、`ctx.plugin(fn)`）是另一套 API，Rust-cordis 暂不支持，不适用本技能。
---

# Cordis 第一性原理：一切皆插件，时空可组合

Rust-cordis 源自论文《一种面向时空可组合性的编程范式》。整个系统建立在两条不可动摇的范式原点之上：

1. **极简通用内核，核心之外一切皆插件。**
   - 内核（`crates/cordis`）与业务完全无关，不执行插件代码，也不理解 HTTP、数据库、认证、模型或界面；它只用同一个 `Context` 维护组件实例、依赖绑定、效应回收与热替换调和。
   - 一切业务能力和系统能力（HTTP、KV、机密、终端日志、文件监听、桌面窗口、前端页面，乃至监管器与网页部署自身的框架装配）都表达为可独立组合、可运行中替换的组件或插件。
   - Rust 动态库在 `cordis-host` 中运行，Python 与 TypeScript 插件是独立进程，浏览器组件经 `http` 插件接入；跨进程统一走 UP/1 协议，共享同一套实例、依赖、效应与状态继承语义。

2. **时空可组合性。**
   - **时间可组合性（可撤销效应）：** 组件运行期间登记的服务、监听器、子组件以及文件、连接、后台任务等副作用，全部归属于当前 `Context` 的效应作用域。组件卸载、依赖失效或热替换时，内核按 LIFO 逆序干净归还全部效应；外部持久化作用由掌握该业务数据的插件保证幂等与一致性。
   - **空间可组合性（反应式依赖与上下文视图）：** 组件只声明需要的服务契约（`inject`）与提供的能力（`provides`），同名接口在不同隔离域（`Realm`）与拦截视图中可指向不同提供方。依赖暂时缺席属于正常状态；提供方上线、下线或换代时，内核按已提交绑定自动完成等待、激活、重连与重建。

## 落地判据：唯一事实所有者与四条推论

为了在多语言、跨进程、并发与热替换场景下不破坏“一切皆插件”和“时空可组合性”，任何设计动手前先回答一个问题：**决定这件事正确结果的事实，由谁唯一掌握？** 掌握事实的一方承担这项职责。

| 事实 | 唯一掌握者 |
|---|---|
| 实例、依赖、加载代、清理时机、替换与回滚；调用来自谁、截止时间、连接是否健康 | 核心 |
| 协议（HTTP、Cookie、会话）与认证结果 | 终结该协议的插件 |
| 业务数据、领域规则、业务状态、任务进度 | 掌握这类数据的那一个业务插件 |
| 机密的值 | secrets 插件 |
| 文件、连接、监听器、后台任务 | 取得它的那个实例 |
| 原生插件的执行线程、动态库租约与映射寿命 | 分组的宿主进程 `cordis-host` |
| 进程划分、分组、淘汰阈值、全局超时与重启参数 | 部署配置 `cordis.toml` |
| Rust-cordis 本身如何运行 | Rust-cordis 的源码 |

以下四条推论直接守住上述范式原点：

1. **职责跟着事实走（守住“一切皆插件”）。** 掌握者承担职责，其他方向它要，不把业务塞进核心。默认一个插件包交付一项完整业务能力；包内用模块组织代码，需要独立依赖或生命周期的单元用 Cordis 组件。拆包条件见[插件边界](references/principles.md#13-插件边界)。
2. **只写自己掌握的事实，只信掌握者给出的事实（守住空间隔离）。** 数据只经所有者，身份只信核心，服务读取沿已提交绑定，机密只在 secrets 服务，结论只由被测代码和真实流程产生。
3. **只占用自己拥有的资源，用完归还（守住时间可组合性）。** 实例资源、调用截止时间、宿主执行线程、插件运行时、事件循环各有主人；用效应绑定寿命，清理有界且逆序退回。
4. **跨越边界只靠带版本的契约（守住独立替换演进）。** 跨插件、跨进程与跨语言交互一律使用带版本的接口与状态模式，不共享表、全局变量或私有格式。

设计、审查或判断“该放在哪里”之前，先读[推论详解](references/principles.md)。它展开每条推论的正反例、“不改内核”的步骤、遇到未覆盖场景的处理方法，以及交付前自查清单。

## 先确认

1. 当前项目根目录有 `cordis.toml`（v0.3.x 推荐 `format_version = 2`，原生插件默认共享 `native` 宿主组），插件由 `cordis-dev` 脚手架生成。
2. 代码若是原版 cordis 写法（从 `cordis` 包导入 `Context`、`ctx.plugin(fn)`），告诉用户 Rust-cordis 尚不能运行，需要改写为本技能的 SDK 写法。
3. 沿用项目已有的语言和接口；`cordis version`、`cordis-dev version` 查看版本。

## 按任务阅读

| 任务 | 阅读 |
|---|---|
| 理解范式与四条推论、划分插件包、判断能力归属 | [推论详解](references/principles.md)、[插件基础](references/plugin-basics.md) |
| 写 Rust 原生插件、配置 `tokio = false` 与 ABI 1.2 | [Rust 接口](references/rust-api.md)、[资源与状态](references/resources-state.md) |
| 写 Python、TypeScript、网页插件与共享 WIT 绑定（`python-sdk` / `ts-sdk`） | [Python](references/python.md)、[TypeScript](references/typescript.md)、[网页与全栈](references/web.md) |
| HTTP 路由、认证、字节流、机密 | [HTTP、流与身份](references/http.md) |
| 调用外部服务或模型、超过 30 秒的长任务、Rust 同步等待 | [调用、超时与长任务](references/calls.md) |
| 替换或回滚后哪些数据会保留、哪些会丢 | [资源与状态](references/resources-state.md)“替换能保住什么” |
| 修改已发布接口、升级主版本、状态格式变化 | [接口版本与升级](references/versioning.md) |
| 方法名来自配置或外部输入 | [按名称调用](references/dynamic.md) |
| 把现有应用迁过来 | [迁移现有应用](references/migration.md) |
| 契约验证与运行中替换 | [验证与部署](references/verification-deployment.md) |
| 打包发布 | [打包与独立运行时](references/packaging.md) |
| 装成服务、共享宿主、压低 `cordis-host` 内存、退役阈值、崩溃恢复 | [生产部署与运维](references/deployment.md) |
| 出现故障（超时卡死、内存偏高、code 8、状态丢失） | [常见故障](references/troubleshooting.md) |
| 在 Rust-cordis 仓库内改 SDK、模板或官方插件 | [维护仓库本身](references/repository.md) |

交付时说明 SDK 版本、构建与运行命令、实际观察到的行为和验证覆盖的范围，并按推论详解末尾的清单自查。
