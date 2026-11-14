---
name: cordis-first-principles
description: Rust-cordis 的第一性原理与插件构建：先判定每项职责的事实由谁唯一掌握，据此把能力放进正确的插件，不改核心；再用 Rust、Python、TypeScript 或网页 SDK 写出插件，处理依赖、效应清理、状态继承、运行中替换、认证与机密、HTTP 路由和发布打包。在 Rust-cordis 上设计新功能、把现有应用迁到 Rust-cordis、拆分服务、判断某项能力该放在哪里、编写或审查插件、处理长任务与超时、升级接口版本、部署为服务或上 Linux 时使用。原版 cordis 或 Koishi 的 JavaScript 插件（`import { Context } from 'cordis'`、`ctx.plugin(fn)`）是另一套 API，Rust-cordis 暂不支持，不适用本技能。
---

# Cordis 第一性原理

Rust-cordis 的核心是一个固定的程序：它不运行插件代码，只管理组件的依赖、加载、效应与替换。一切功能都在插件里。Rust 动态库在 cordis-host 中运行；Python 与 Node 插件以独立进程接入；浏览器组件经 http 插件转发的 WebSocket 接入同一个核心。插件与核心之间只走 UP/1 协议。

## 一个问题

做任何设计之前先问：**决定这件事正确结果的事实，由谁唯一掌握？** 掌握事实的一方承担这项职责。

| 事实 | 掌握者 |
|---|---|
| 实例、依赖是否满足、加载与清理时机、替换与回滚、调用来自哪个实例或页面 | 核心 |
| 协议（HTTP、Cookie、会话）、认证结果 | 终结该协议的插件 |
| 业务数据、领域规则、业务状态 | 业务插件 |
| 机密的值 | secrets 插件，按实例授权 |
| 代码在哪个进程运行、哪些插件共享故障域 | 部署配置（group） |

由此得到六条判据：

1. 核心不运行任何插件代码。
2. 只有一个内核，实例、依赖、生命周期、替换与回滚只由它决定。
3. 核心之外一切皆插件。能由插件承担的职责，不进入核心。
4. 协议与领域语义归终结它的插件，核心只转交不透明数据。
5. 先复用已有机制，不为单个使用方开特例；为某个应用提出的需求，按任何应用都适用的通用能力设计。
6. 机密与业务载荷由插件读取、解释和保存；核心不解析它们，机密也不放进环境变量。

## 不改内核

觉得“这个功能得改核心才能做”时，按顺序走：

1. 用上面的问题找出事实的掌握者。几乎所有情况下答案都是某个插件。
2. 找已有机制：效应、依赖注入、`scoped` 与 `intercept` 元数据、资源对象作为参数转交、动态调用、`cordis.manage@1` 授权、http 路由、secrets 服务。
3. 用插件实现。需要别的插件配合时，定义一个带版本的接口。
4. 只有能证明相关事实只有核心掌握（例如“一次调用来自哪个连接”）时，才把需求写成通用能力，交给 Rust-cordis 维护者评审；不要在自己的项目里修改或分叉核心。

具体推论、错误写法与正确写法见[职责放在哪里](references/principles.md)。设计新服务、认证、机密、跨机器协作时必读。

## 迁移现有应用

把已有应用（自研插件系统、单体后端、脚本加守护进程）迁到 Rust-cordis，是按上面的问题把每项职责重新归位，不是搬代码。旧系统的职责只有三种去处：

1. 实例、依赖、加载、替换类的职责删除，由核心接管。自研加载器、插件清单、注册表、热重载都不搬进来，也不在插件里重写。
2. 协议、机密、文件监视交给官方插件 http、secrets、watch。
3. 业务数据与规则写成业务插件，按事实拆分，接口带版本。

迁移前先读[迁移现有应用](references/migration.md)：规则 M-1 到 M-9、旧写法与新写法对照、迁移步骤和完成标志。

## 先确认

1. 当前项目是 Rust-cordis 项目：根目录有 `cordis.toml`，插件由 `cordis-dev` 脚手架生成，Rust 插件以 `cordis_sdk::export_plugin!` 导出。
2. 用户的代码若是原版 cordis 写法（从 `cordis` 包导入 `Context`、`ctx.plugin(fn)`、`ctx.on('ready')`），告诉用户 Rust-cordis 尚不能运行原版插件，需要改写为本技能的 SDK 写法。原版兼容的设计见 Rust-cordis 公开仓库（https://github.com/Proton1917/Rust-Cordis）的 `docs/17-upstream-ecosystem-compat.md`，尚未实现。
3. 用 `cordis version` 查看核心版本，`cordis-dev version` 查看开发工具。沿用项目已有的语言和接口。

## 选择入口

新项目用 `cordis new demo`。插件名以小写字母开头，只含小写字母、数字和短横线。

先按职责选形态，再选命令：

| 形态 | 选它的理由 | 代价 |
|---|---|---|
| Rust 原生插件 | 计算密集或调用频繁的路径；同组原生插件之间直接调用，不经过核心 | 同组共享故障域；需要编译 |
| Python 进程插件 | 依赖 Python 生态（模型、数据处理、科学计算库） | 每个插件包一个解释器进程；所有调用经核心转交；一个事件循环，阻塞 I/O 会拖住整个进程 |
| TypeScript 进程插件 | 依赖 Node 生态，或与网页共用类型与代码 | 同 Python |
| 网页组件 | 界面；运行在浏览器里，每个页面一个子实例 | 只能调用对页面授权的服务 |

项目已经用某种语言实现了相关接口时，沿用它；跨语言只经版本化接口。插件跑在哪个进程、哪些插件合组，是部署决定，不写进代码，见[生产部署与运维](references/deployment.md)“分组与容量”。

| 命令 | 组件形态 | 按需阅读 |
|---|---|---|
| `cordis new-plugin hello --rust` | Rust 原生插件 | [Rust 接口](references/rust-api.md)、[资源与状态](references/resources-state.md) |
| `cordis new-plugin hello --python` | Python 进程插件 | [Python 插件](references/python.md) |
| `cordis new-plugin hello --ts` | Node 中的 TypeScript 插件 | [TypeScript 插件](references/typescript.md) |
| `cordis new-plugin hello --web` | 浏览器组件 | [网页与全栈插件](references/web.md) |
| `cordis new-plugin hello --rust --web` | 后端加浏览器组件；后端也可用 `--python`、`--ts` | 同上 |
| `cordis new-plugin api --rust --http` | HTTP 处理方；也可用 `--python`、`--ts` | [HTTP、流与身份](references/http.md) |

版本要求：Rust 插件与接口库 Rust 1.85 及以上；Python SDK 3.11 及以上；TypeScript 进程 SDK Node 22 及以上。

其他按需阅读：

| 情况 | 阅读 |
|---|---|
| 调用外部服务、模型或任何可能超过 30 秒的工作 | [调用、超时与长任务](references/calls.md)（必读） |
| 修改已发布的接口、升级主版本、状态格式变化 | [接口版本与升级](references/versioning.md) |
| 方法名来自配置或外部输入 | [按名称调用](references/dynamic.md) |
| 开发时运行、契约验证与替换 | [验证与部署](references/verification-deployment.md) |
| 打包发布 | [打包与独立运行时](references/packaging.md) |
| 装成服务长期运行、分组与容量、日志、崩溃恢复、备份、上 Linux | [生产部署与运维](references/deployment.md) |

## 每个插件都遵守的规则

1. **声明与实际一致。** 组件的必需与可选依赖写在 inject，提供的服务写在 provides，监听的事件写在 listens，迁移的状态写模式名与版本范围。未声明的接口取不到。
2. **resolve 只检查配置，apply 负责激活。** 必需依赖缺席时核心不调用 apply，实例保持 Pending；依赖恢复后自动加载。
3. **资源归属当前实例。** 文件、连接、监听器在效应中取得，清理登记在同一效应。后台工作用 SDK 的任务入口（Rust `ctx.spawn`/`ctx.spawn_thread`，Python 与 TypeScript `context.task`），不要用脱离实例的线程、全局 `tokio::spawn`、裸 `asyncio.create_task` 或 `setInterval`，否则替换后旧代码继续运行。
4. **业务状态经导出器迁移。** 写入时在同一临界区检查冻结标志再修改；导出器读同一份状态。
5. **调用服务或派发事件前释放本地锁**，让重入的回调能继续执行。
6. **跨进程事件只是通知。** 需要返回值的交互用服务方法。
7. **替换保持兼容。** 逻辑插件名、组件名、接口主版本不变；追加方法提升接口次版本，追加在末尾。不兼容修改发布新主版本，过渡方法见[接口版本与升级](references/versioning.md)。
8. **公开接口用 WIT 或共享 Rust 接口 crate 管理。** 三种语言的绑定由 `cordis idl gen` 从同一份 WIT 生成。
9. **服务方法不等待长工作。** 经核心转交的调用缺省 30 秒截止，同一连接连续 3 次超时，核心会终止提供方进程。可能变慢的工作改为提交、执行、查询三步，见[调用、超时与长任务](references/calls.md)。Python 与 TypeScript 插件中不做同步阻塞 I/O。

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
| 调用返回错误码 6，随后提供方进程被终止、依赖方转 Pending | 服务方法等待了长工作；或 Python、TypeScript 插件中的阻塞 I/O 卡住了事件循环 | 规则 9 |
| 插件反复重启后停在失败状态 | 进程崩溃，重启次数已耗尽 | 看日志修复原因，见 [生产部署与运维](references/deployment.md)“崩溃与恢复” |

## 交付记录

说明使用的 SDK 版本、构建与运行命令、实际观察到的行为（服务调用、清理、状态继承、替换结果）以及验证覆盖的范围。交付前按[职责放在哪里](references/principles.md)末尾的自查清单核对。

在 Rust-cordis 仓库内修改 SDK、模板或官方插件时，另读[维护仓库本身](references/repository.md)。
