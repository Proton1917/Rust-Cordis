# 插件基础

## 选形态

一切业务能力都在插件里，形态只决定执行位置与故障边界，不改变组件、依赖、效应和状态继承语义。

| 形态 | 适合 | 代价与注意 |
|---|---|---|
| Rust 原生插件（共享 `native` 宿主） | 高频互调、计算密集或核心本地服务；同组且布局摘要匹配时走 ABI 1.2 类型化直调，不经过核心 | 同组共享进程故障域与执行线程；各 `cdylib` 独立静态链接自身依赖；不需要第三方 Tokio 生态的插件应显式设置 `tokio = false` 以省去库级运行时开销 |
| Rust 原生插件（独立 `group`） | 需要 Rust 性能且必须故障隔离的重型插件（外部 C 库、重型计算、不稳定实验模块） | 多一个 `cordis-host` 进程（空闲约 3.6–4.5 MiB），跨组调用走 UP/1 |
| Python 进程插件 | 依赖 Python 生态（AI/ML 模型、数据处理、科学计算） | 每个插件包一个独立解释器进程；地址空间完全隔离，无 `cdylib` ABI 与映像残留问题；调用经核心转交，单个事件循环 |
| TypeScript 进程插件 | 依赖 Node 生态，或与网页共用类型与代码 | 同 Python，地址空间完全隔离 |
| 网页组件 | 浏览器界面；每个页面连接一个子实例 | 只能调用对页面显式授权的服务 |

**选型原则：** 业务调用频率达不到每秒数万次标量读写时，独立进程插件或普通跨组调用已经足够快，且完全避开动态库地址空间共享、依赖重复静态链接和 `dlclose` 映像残留；真正需要纳秒级同进程直调的热点模块再放进共享原生宿主，并用同版 `cordis-sdk` 编译发布。

项目已经用某种语言实现了相关接口时，沿用它；跨语言只经版本化接口。插件跑在哪个进程、和谁合组由 `cordis.toml` 决定（见[生产部署与运维](deployment.md#分组与容量)）。

## 生成

新项目用 `cordis new demo`（v0.3.x 生成格式 2 配置，默认共享 `native` 组）。插件名以小写字母开头，只含小写字母、数字和短横线。

| 命令 | 组件形态 | 下一步阅读 |
|---|---|---|
| `cordis new-plugin hello --rust` | Rust 原生插件 | [Rust 接口](rust-api.md)、[资源与状态](resources-state.md) |
| `cordis new-plugin hello --python` | Python 进程插件 | [Python 插件](python.md) |
| `cordis new-plugin hello --ts` | Node 中的 TypeScript 插件 | [TypeScript 插件](typescript.md) |
| `cordis new-plugin hello --web` | 浏览器组件 | [网页与全栈插件](web.md) |
| `cordis new-plugin hello --rust --web` | 后端加浏览器组件；后端也可用 `--python`、`--ts` | 同上 |
| `cordis new-plugin api --rust --http` | HTTP 处理方；也可用 `--python`、`--ts` | [HTTP、流与身份](http.md) |

版本要求：Rust 插件与接口库 Rust 1.85 及以上；Python SDK 3.11 及以上；TypeScript 进程 SDK Node 22 及以上。

## SDK 的基本要求

1. **声明与实际一致。** 必需与可选依赖写在 `inject`，提供的服务写在 `provides`，监听的事件写在 `listens`，迁移的状态写模式名与版本范围。未声明的接口取不到。
2. **resolve 只检查配置，apply 负责激活。** 必需依赖缺席时核心不调用 `apply`，实例保持 Pending；依赖恢复后自动加载。
3. **写业务状态时检查冻结。** 在同一临界区先检查冻结标志再修改；导出器读同一份状态。冻结期间写入返回错误码 4。
4. **三种语言的绑定由 `cordis idl gen` 从同一份 WIT 生成。** 独立脚本工程用 `--lang python` / `--lang ts`；运行目录已带同版 SDK 时，用 `--lang python-sdk` / `--lang ts-sdk` 共享 SDK 内置 WIT 运行时，大幅减少生成代码体积。
5. **替换时保持逻辑插件名、组件名和接口主版本不变。**
