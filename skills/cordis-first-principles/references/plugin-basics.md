# 插件基础

## 选形态

| 形态 | 适合 | 代价 |
|---|---|---|
| Rust 原生插件 | 计算密集或调用频繁的路径；同组原生插件之间直接调用，不经过核心 | 同组共享故障域与执行线程；需要编译 |
| Python 进程插件 | 依赖 Python 生态（模型、数据处理、科学计算） | 每个插件包一个解释器进程；调用经核心转交；单个事件循环 |
| TypeScript 进程插件 | 依赖 Node 生态，或与网页共用类型与代码 | 同 Python |
| 网页组件 | 界面；每个页面一个子实例 | 只能调用对页面授权的服务 |

项目已经用某种语言实现了相关接口时，沿用它；跨语言只经版本化接口。插件跑在哪个进程、和谁合组由部署决定（见[生产部署与运维](deployment.md)“分组与容量”）。

## 生成

新项目用 `cordis new demo`。插件名以小写字母开头，只含小写字母、数字和短横线。

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

这几条是 SDK 的用法：

1. **声明与实际一致。** 必需与可选依赖写在 inject，提供的服务写在 provides，监听的事件写在 listens，迁移的状态写模式名与版本范围。未声明的接口取不到。
2. **resolve 只检查配置，apply 负责激活。** 必需依赖缺席时核心不调用 apply，实例保持 Pending；依赖恢复后自动加载。
3. **写业务状态时检查冻结。** 在同一临界区先检查冻结标志再修改；导出器读同一份状态。冻结期间写入返回错误码 4。
4. **三种语言的绑定由 `cordis idl gen` 从同一份 WIT 生成。**
5. **替换时保持逻辑插件名、组件名和接口主版本不变。**
