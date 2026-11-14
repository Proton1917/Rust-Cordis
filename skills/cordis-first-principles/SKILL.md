---
name: cordis-first-principles
description: Rust-cordis 的第一性原理与插件构建：先判定每项职责的事实由谁唯一掌握，据此把能力放进正确的插件，不改核心；再用 Rust、Python、TypeScript 或网页 SDK 写出插件，处理依赖、效应清理、状态继承、运行中替换、认证与机密、长任务与超时、接口升级、HTTP 路由、发布打包和生产部署。在 Rust-cordis 上设计新功能、把现有应用迁到 Rust-cordis、拆分服务、判断某项能力该放在哪里、编写或审查插件时使用。原版 cordis 或 Koishi 的 JavaScript 插件（`import { Context } from 'cordis'`、`ctx.plugin(fn)`）是另一套 API，Rust-cordis 暂不支持，不适用本技能。
---

# Cordis 第一性原理

Rust-cordis 的核心不运行插件代码，只管理组件的依赖、加载、效应与替换；一切功能都在插件里。Rust 动态库在 cordis-host 中运行，Python 与 Node 插件是独立进程，浏览器组件经 http 插件接入。插件与核心之间只走 UP/1 协议。

## 一个问题

做任何设计之前先问：**决定这件事正确结果的事实，由谁唯一掌握？** 掌握事实的一方承担这项职责。

| 事实 | 唯一掌握者 |
|---|---|
| 实例、依赖、加载与清理时机、替换与回滚；调用来自谁、截止时间、连接是否健康 | 核心 |
| 协议（HTTP、Cookie、会话）与认证结果 | 终结该协议的插件 |
| 业务数据、领域规则、业务状态、任务进度 | 掌握这类数据的那一个业务插件 |
| 机密的值 | secrets 插件 |
| 文件、连接、监听器、后台任务 | 取得它的那个实例 |
| 原生插件的执行线程 | 分组的宿主进程 cordis-host |
| 进程划分、分组、全局超时与重启参数 | 部署配置 `cordis.toml` |
| Rust-cordis 本身如何运行 | Rust-cordis 的源码 |

## 四条推论

本技能的所有规则都由这四条推出：

1. **职责跟着事实走。** 掌握者承担职责，其他方向它要；不为单个使用方开特例，不改核心。
2. **只写自己掌握的事实，只信掌握者给出的事实。** 数据只经所有者，身份只信核心，机密只在 secrets 服务，结论只由被测代码和真实流程产生。
3. **只占用自己拥有的资源，用完归还。** 实例资源、调用截止时间、执行线程、事件循环各有主人。
4. **跨越边界只靠带版本的契约。** 接口带版本，不共享表、全局变量或私有格式。

设计、审查或判断“该放在哪里”之前，先读[推论详解](references/principles.md)。它包括每条推论的展开与正反例、“不改内核”的步骤、遇到本技能没写到的情况怎么办，以及交付前自查清单。

## 先确认

1. 当前项目根目录有 `cordis.toml`，插件由 `cordis-dev` 脚手架生成。
2. 代码若是原版 cordis 写法（从 `cordis` 包导入 `Context`、`ctx.plugin(fn)`），告诉用户 Rust-cordis 尚不能运行，需要改写为本技能的 SDK 写法。
3. 沿用项目已有的语言和接口；`cordis version`、`cordis-dev version` 查看版本。

## 按任务阅读

| 任务 | 阅读 |
|---|---|
| 选插件形态、生成插件、SDK 基本要求 | [插件基础](references/plugin-basics.md) |
| 写 Rust 插件 | [Rust 接口](references/rust-api.md)、[资源与状态](references/resources-state.md) |
| 写 Python、TypeScript、网页插件 | [Python](references/python.md)、[TypeScript](references/typescript.md)、[网页与全栈](references/web.md) |
| HTTP 路由、认证、字节流、机密 | [HTTP、流与身份](references/http.md) |
| 调用外部服务或模型、可能超过 30 秒的工作、Rust 中等待异步调用 | [调用、超时与长任务](references/calls.md) |
| 替换或回滚后哪些数据会保留、哪些会丢 | [资源与状态](references/resources-state.md)“替换能保住什么” |
| 修改已发布接口、升级主版本、状态格式变化 | [接口版本与升级](references/versioning.md) |
| 方法名来自配置或外部输入 | [按名称调用](references/dynamic.md) |
| 把现有应用迁过来 | [迁移现有应用](references/migration.md) |
| 契约验证与运行中替换 | [验证与部署](references/verification-deployment.md) |
| 打包发布 | [打包与独立运行时](references/packaging.md) |
| 装成服务、分组与容量、日志、崩溃恢复、上 Linux | [生产部署与运维](references/deployment.md) |
| 出现故障 | [常见故障](references/troubleshooting.md) |
| 在 Rust-cordis 仓库内改 SDK、模板或官方插件 | [维护仓库本身](references/repository.md) |

交付时说明 SDK 版本、构建与运行命令、实际观察到的行为和验证覆盖的范围，并按推论详解末尾的清单自查。
