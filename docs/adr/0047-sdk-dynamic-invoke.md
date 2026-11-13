# ADR-0047 SDK 的动态调用

| 项 | 内容 |
|---|---|
| 状态 | 已决定，M23 实施依据（设计方 2026-11-13）；在 M22 复核通过并发布 v0.1.0 之后实施 |
| 影响 | crates/cordis-sdk（必要时 cordis-sdk-macros）；sdks/python、sdks/typescript 的对齐；docs/guide.md；skills/cordis-plugin |
| 不影响 | 核心进程的全部 crate、cordis-abi 与 ABI v1 布局、UP/1 |

## 1 问题

Rust 插件只能经 `context.get::<dyn I>()` 取得类型化代理，调用方法名在编译期固定。有些插件要在运行时按名称决定调用哪个服务的哪个方法：

| 使用方 | 场景 | 现状 |
|---|---|---|
| MathBook worker | 执行配方，配方中的步骤写明服务名与方法名 | 手写分发表 `backend/crates/service-dispatch/src/lib.rs`，约 100 行，每新增一个方法都要改这张表（MathBook 提交 53b569a） |
| MathBook `/api` | 把 HTTP 路径映射到服务方法 | 等待本 ADR，暂未实现 |
| 任意网关、调度器、管理工具 | 按配置转发调用 | 同上，只能手写分发表 |

核心已有按方法名的通用调用入口：DOC-12 的 V-2，即对象的 `invoke`，参数为方法名加 CBOR 参数。跨进程调用与 Python、TypeScript 插件都走这个入口。Rust SDK 只在内部使用它（类型化代理在 typed 槽位不可用时回退到它），没有开放给使用方。

## 2 职责放置（AGENTS.md“第一性原理”）

| 事实 | 唯一掌握者 | 放置 |
|---|---|---|
| 某个组件依赖哪些服务、这些服务当前是否可用 | 内核（经组件的 `inject` 声明） | 不变 |
| 按名称选择方法并编码参数 | 使用方插件 | SDK 的使用方接口 |
| 方法名到实现的分派 | 提供方的通用入口 | 不变（V-2） |

动态调用只把已有的通用入口开放给使用方，不新增核心职责，依据第一性原理第 3、5 条。

## 3 决定

1. Rust SDK 增加：
   - `Context::get_dynamic(interface: &str) -> Result<DynamicService>`。`interface` 的写法与 `inject` 中的声明相同，例如 `"mbs.workflow@1.0"`；
   - `DynamicService::invoke(method: &str, args: CborValue) -> Result<CborValue>`；
   - 辅助方法 `invoke_as<A: Serialize, R: DeserializeOwned>(method, args: &A) -> Result<R>`；
   - 同步或异步的形式与 SDK 现有类型化代理的约定一致。
2. 取得规则与类型化 `get` 完全相同：
   - 普通服务只能取得组件在 `inject` 中已声明的接口；未声明的接口返回错误，错误类别与类型化 `get` 取得未声明依赖时相同；
   - 核心内置服务（由核心自己提供的服务，例如 `cordis.runtime@1.3`；以现有类型化取得规则中的内置清单为准，不按名称前缀判断，`cordis.http@1.0`、`cordis.secrets@1.0` 等由插件提供的接口属于普通服务）沿用既有的类型化取得规则，不要求在 `inject` 中声明（M23 实施中确认，设计方 2026-11-13）；这与上游内置服务直接挂在上下文上的语义一致；
   - 依赖追踪、`Pending`、热替换与回滚的行为与类型化代理完全相同。
3. 参数编码与 V-2 通用调用相同：位置参数组成 CBOR 数组。未知方法返回的错误与提供方通用入口对未知方法的错误相同。`list<u8>` 按 ADR-0039 的 M22 修订编码为字节串。
4. `DynamicService` 支持 `scoped` 视图：取得方式与类型化代理相同，提供方收到的元数据（包括 `cordis.caller`）一致。
5. 同组原生提供方也经通用入口调用，不走 typed 槽位。需要直接调用速度的场景继续用类型化代理；guide 写明两者的取舍。
6. Python 与 TypeScript SDK：先核对是否已有等价的动态调用。已有的，在 guide 中列出对应写法；没有的，按第 1–4 条补齐，名称采用各语言的惯例。
7. 资源（V-2 中 tag 60000 的服务对象引用）的所有权（M23 实施前确认，设计方 2026-11-13）：
   - `CborValue` 是纯数据，不能持有资源引用。`invoke` 只处理不含资源的参数与结果；
   - `invoke` 的结果含资源时，SDK 先按现有规则释放这些引用，再返回参数错误，错误信息提示改用 `invoke_as`；不得把即将失效的编号交给使用方；
   - `invoke` 的参数含 tag 60000 时同样返回参数错误，使用方不能用裸编号构造资源引用；
   - 含资源的调用使用 `invoke_as`：参数类型与结果类型中的 `Service` 等资源字段经现有 `CborEncoder`/`decode_in` 编解码，在结果缓冲释放前导入，拥有关系与类型化代理相同；
   - `Inject` 增加按接口名声明依赖的写法（`with_name`、`optional_name`），校验规则与类型化声明相同，供没有 Rust 接口类型的动态使用方声明依赖。
8. 核心、ABI、UP/1 不改动。实施中若发现必须改动，先停下，把原因与改动范围交设计方确认。

## 4 验收

| 编号 | 场景 | 通过的判据 |
|---|---|---|
| `AU-27` | 同一个 Rust 使用方分别经类型化代理与 `DynamicService` 调用 kv@1 的全部方法；提供方依次为同组原生、跨组原生、Python、TypeScript | 两种调用的结果与错误逐项相等 |
| `AU-28` | 取得未在 `inject` 中声明的接口；调用不存在的方法；参数个数或类型错误 | 依次得到未声明依赖错误、未知方法错误、参数错误；提供方与核心不受影响 |
| `AU-29` | 动态调用持续进行时，替换提供方、使替换失败回滚、卸下提供方 | 与类型化代理在同样操作下的行为一致：替换后调到新版本；回滚后调到旧版本；卸下后使用方转 `Pending` |
| `AU-27a` | 提供方返回 `cordis.byte-stream@1.0` 资源：分别经 `invoke` 与 `invoke_as` 调用；`invoke` 的参数中放入伪造的 tag 60000 | `invoke` 得到参数错误，提供方观察到该资源被释放；`invoke_as` 取得可读的字节流，读完后释放；伪造编号得到参数错误，提供方不受影响 |
| `AU-30` | 一个 `scoped` 提供方 | 动态调用与类型化调用时，提供方收到的 `cordis.caller` 相同 |
| `AU-8` 复查 | 核心组成 | 相对 M22 提交，核心 crate 与 cordis-abi 无改动 |

另外需要：原有测试、177 条上游用例、unsafe 审计、fmt、clippy 全部通过；guide 新增“动态调用”一节，写明它与类型化代理的取舍；skill 同步。

## 5 后果

1. MathBook 的 service-dispatch 可以换成配置驱动的 `get_dynamic` 加 `invoke_as`，新增方法时不再修改代码。`/api` 的路径映射用同一方式实现。
2. 动态调用失去编译期的方法名与类型检查，错误推迟到运行时。guide 建议只在名称确实来自配置或外部输入时使用。
3. 纯新增接口，v0.1.0 的插件不受影响，作为 v0.2.0 发布。
