# ADR-0055：SDK 为原生插件提供库级 tokio 执行上下文

| 项 | 值 |
|---|---|
| 状态 | 已实现 |
| 日期 | 2026-11-16 |
| 依据 | DOC-11 §6 `PR-1`–`PR-3`，DOC-12 §5、§8、§10，ADR-0012，ADR-0051，DOC-13 §7 |
| 修订 | DOC-11 `PR-3`；DOC-13 §7 依赖白名单的 `tokio` 行 |

## 问题与实测

原生插件编译为独立的动态库。每个动态库静态链接自己的一份 tokio，tokio 的“当前运行时”保存在该副本的线程局部变量中。cordis-host 用自己的 tokio 运行时轮询插件交出的 Future；插件代码在宿主线程上执行时，插件那份 tokio 看不到任何运行时。

DOC-11 `PR-3` 把这一事实写成插件作者的义务：需要 tokio 网络或文件接口的插件在效应中自建运行时，并避免调用全局 `tokio::spawn`。违反的结果是运行时 panic：`there is no reactor running, must be called from the context of a Tokio 1.x runtime`。

一个下游应用迁移到 Rust-cordis 时出现了以下现象：

1. 远程并发验证中，HTTP 请求在构造阶段因缺少 tokio 上下文而失败，修正后才进入正式测量。
2. 应用自行编写了网络运行时包装：在效应中创建单线程 tokio 运行时，并在每次轮询业务 Future 前调用 `Handle::enter()`。使用网络客户端的两个插件各自创建这样的运行时。

第 2 条中的包装逻辑与插件业务无关，每个使用 reqwest、hyper 或 tokio 计时器的插件都需要重复编写。规则 `PR-3` 本身也只有在违反后才会以 panic 暴露。

## 职责放置

“插件代码运行时哪一份 tokio 处于当前状态”这一事实只存在于插件动态库内部：

1. 宿主无法提供运行时。宿主的 `tokio::runtime::Handle` 与插件中的同名类型属于不同的编译副本，不能跨动态库传递（DOC-12 §2 只允许 C 布局对象跨边界）。
2. 内核与 runtime 不运行插件代码（`Q0`），也不掌握插件的 tokio 副本。
3. 插件代码进入的唯一入口是 cordis-abi 的源侧边界函数 `boundary` 与 `release_boundary`（DOC-12 §5）。cordis-abi 同时编译进宿主和每个插件，它本身不应依赖 tokio。

因此：

- **cordis-abi** 提供一个与执行器无关的源侧钩子 `Ambient`，边界函数在执行用户代码前进入该钩子。宿主进程不登记钩子，其行为保持不变。
- **cordis-sdk** 编译进插件，掌握插件自己的 tokio 副本。它创建库级运行时并登记钩子。

## 选项

| 选项 | 内容 | 判断 |
|---|---|---|
| A | 保持 `PR-3`，只改进文档 | 每个插件继续重复编写包装；违反后仍在运行时 panic |
| B | 宿主把自己的 tokio 运行时交给插件 | 不可行：tokio 类型与线程局部变量按编译副本隔离，无法跨动态库共享 |
| C | SDK 为每个插件库懒创建一个 tokio 运行时，边界函数自动进入其上下文 | 采用 |
| D | 每次调用临时创建运行时 | 每次调用都要创建线程和驱动；连接池等跨调用资源无法复用 |

判据：

1. 插件作者无需记忆额外规则，常用 tokio 生态库开箱可用。
2. 不改变 ABI v1 布局，已编译的插件和宿主不受影响。
3. 资源开销有上界，库退役后不再增长。

## 决定

### cordis-abi：源侧环境钩子

新增模块 `ambient`：

```rust
pub struct Ambient {
    /// 在插件的执行上下文中运行一次 operation。
    pub enter: fn(&mut dyn FnMut()),
    /// 插件库的描述符释放后调用一次。
    pub release: fn(),
}
pub fn install(ambient: Ambient) -> bool;
```

- 钩子保存在 cordis-abi 的进程级静态变量中。每个插件动态库有自己的一份 cordis-abi，钩子只作用于登记它的那个库。
- `boundary` 与 `release_boundary` 在 `catch_unwind` 内部、执行用户代码之前进入钩子；钩子未登记时直接执行。钩子内的 panic 与用户代码的 panic 一样转换为 code 5。
- 描述符释放函数 `release_descriptor` 在析构描述符之后调用 `release`。

### cordis-sdk：库级 tokio 运行时

新增 cargo feature `tokio`，默认开启。开启时：

1. `export_plugin!` 生成的入口函数在握手中登记钩子。
2. 第一次进入钩子时创建运行时：`tokio::runtime::Builder::new_multi_thread()`，1 个工作线程，`enable_all()`，线程名 `cordis-plugin-tokio`。创建失败时输出一条诊断，之后的调用不进入上下文，行为与旧版相同。
3. `enter` 对每次边界调用执行 `handle.enter()`。插件代码中的 `tokio::spawn`、`tokio::time`、reqwest 等因此找到运行时。
4. `release` 用 `shutdown_background()` 非阻塞关闭运行时，并把状态置为已释放。已释放后不会再创建新的运行时。

插件可用 `default-features = false` 关闭该 feature，此时不依赖 tokio，行为与 0.2.1 相同。

### 关闭时机的依据

宿主退役一个库时，先析构组件与描述符，再等待库租约归零（cordis-host `Host::retire`）。描述符释放发生在核心完成该库全部实例清理之后；此时仍可能存在少量收尾调用。这些调用如果使用 tokio，会因运行时已关闭得到 I/O 错误或 panic；panic 由 `boundary` 转换为 code 5，不会出现未定义行为。在租约归零时关闭需要新增 ABI 回调，收益只覆盖上述收尾窗口，本决定不采用。

### 与 `PR-1`、`PR-2` 的关系

全局 `tokio::spawn` 派发的任务属于插件库，不属于实例效应。实例停用时它们不会被取消，库退役时随运行时关闭而终止。因此：

- 需要随实例停用而结束的后台工作，继续使用 `ctx.spawn`（`PR-1`）。
- 第三方库内部派发的任务（例如 HTTP 连接池的连接任务）可以依赖库级运行时。

### 修订 DOC-11 `PR-3`

`PR-3` 改为：插件可以直接使用 tokio 接口，SDK 提供库级运行时上下文；随实例停用而结束的后台任务仍通过 `ctx.spawn` 派发。插件在效应中自建运行时的旧写法继续有效。

## 影响与验收

| 项 | 内容 |
|---|---|
| ABI | 布局不变，仍为 v1.1；钩子只在插件一侧生效 |
| 兼容 | 新 SDK 编译的插件在任何 ABI 1.x 宿主上都得到 tokio 上下文；旧插件行为不变 |
| 版本 | cordis-abi 0.1.2，cordis-sdk 0.2.2 |
| 依赖 | cordis-sdk 在 `tokio` feature 下依赖 tokio（`rt-multi-thread`）；DOC-13 §7 白名单的使用者增加 `cordis-sdk` |
| 资源 | 每个插件库映像一个 tokio 工作线程；退役映像的线程在描述符释放时结束 |

验收：

1. SDK 回环测试：组件在不处于任何 tokio 上下文的系统线程上被调用，组件内的 `tokio::spawn` 与 `tokio::time::sleep` 正常完成。
2. 同一测试中描述符释放后，运行时进入已释放状态，再次进入边界不会创建新运行时。
3. 未登记钩子时 `boundary` 的行为保持不变：既有 ABI 测试与 Miri 用例通过。
4. 真实宿主场景：原生夹具在方法中使用 `tokio::time::sleep` 与 `tokio::spawn`，经 cordis-host 调用成功。
5. 格式、clippy、全量测试、177 条用例矩阵、unsafe 审计通过。
