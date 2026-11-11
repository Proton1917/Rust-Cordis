# ADR-0006 事件的类型化

| 项 | 内容 |
|---|---|
| 状态 | 已接受（2026-11-11，M2 实现） |
| 影响 | `DOC-02` §6、§7；`DOC-01` `S-EVT-*` |

## 问题

`UPSTREAM` 的事件以字符串或符号为名，监听器的签名通过 TypeScript 的 `Events` 接口声明。同一个事件可以用五种方式派发（`emit`、`parallel`、`serial`、`bail`、`waterfall`），监听器的同步或异步形式由调用约定决定。Rust 通过函数签名区分同步回调与异步回调，需要确定事件身份、派发方式与监听器签名的关系。

## 选项

| 选项 | 内容 | 代价 |
|---|---|---|
| A | 字符串名，参数与返回值为 `Box<dyn Any>` | 类型错误推迟到运行期 |
| B | 事件是类型 `E: Event`；每个事件在声明时确定所属的派发族，监听器签名随之确定 | 跨派发族的逻辑事件需要分别定义类型 |
| C | 事件是类型；全部监听器采用异步形式 | `emit`、`bail`、`internal/get`、`internal/config` 的同步返回契约需要调整 |

## 判据

1. `DOC-00` `P5`：能交给类型系统的检查交给类型系统。
2. `DOC-01` `S-EVT-4`、`S-EVT-6`、`S-EVT-8`：`emit`、`bail` 同步执行；`waterfall` 同步调用中间件并交回返回值，该值可以是异步任务。内核事件 `internal/config`、`internal/listener`、`internal/update` 分别以 `waterfall`、`bail`、`waterfall` 派发。
3. `UPSTREAM` 用例对五种方式各有独立的断言，五种方式都要存在。

## 决定

选 B。`Event` 以 `Args`、`Output` 声明参数及返回值，以 `Kind` 关联类型选定派发族。`Args` 实现 `Clone + Send + Sync`，异步派发与瀑布派发因此可把参数交给拥有独立生命周期的监听器。`NAME` 提供错误消息和观察事件中的名称，登记身份取 `TypeId::of::<E>()`。

| `Kind` | 监听器 | 登记接口 | 派发方法 |
|---|---|---|---|
| `SyncKind` | `Fn(&E::Args) -> Option<E::Output>` | `on`、`on_with`、`once`、`once_with` | `emit` 忽略返回值；`bail` 返回首个 `Some` |
| `AsyncKind` | `Fn(E::Args) -> Task<Result<Option<E::Output>>>` | `on_async`、`once_async` | `parallel` 等待全部结果并聚合错误；`serial` 逐个等待，返回首个 `Some` |
| `WaterfallKind` | `Fn(E::Args, Next<E>) -> Result<E::Output>` | `on_waterfall`、`once_waterfall` | `waterfall` 返回 `Result<E::Output>` |

`SyncEvent`、`AsyncEvent`、`WaterfallEvent` 分别由相应 `Kind` 自动实现。单个事件类型的 `Kind` 唯一，登记和派发因此使用同一种回调表示。

`UPSTREAM` 的非空结果对应 `Some`，包括 `Some(0)`；空结果对应 `None`。异步监听器通过 `Result` 表达失败；`parallel` 的错误携带全部失败成员及事件名称。调用方驱动的监听器发生 panic 时按 `A-9` 向调用方传播。

`Next::run(&self)` 返回 `Result<E::Output>`。每层延续及其克隆共享一次调用权，重复调用返回 `next() called multiple times` 错误。异步瀑布采用 `E::Output = Task<Result<T>>`：`next.run()?` 取得任务，`.await?` 取得结果。每次派发独立保存快照与延续，因而支持嵌套异步调用。

五种派发均提供 `_filtered` 变体，接收 `Option<&dyn Fn(&Context) -> bool>`。过滤器接收监听器登记上下文，`global` 监听器直接进入快照。`internal/dispatch` 的记录包含模式、事件类型、名称、参数、发起上下文和过滤状态。

`internal/listener` 接收 `ListenerRegistration`，返回 `Option<Effect>`。接管方通过 `register(options)` 登记实际监听器，也可把它交给父效应的 `adopt`；登记请求保留事件类型、回调、上下文、选项和一次性属性。该路径继续通过 `Context::effect` 跟踪恢复。

监听器快照在锁内创建，过滤、调用以及释放在锁外进行。一次性监听器通过原子移除状态取得执行权，先启动其效应释放再调用回调。并发派发因此共用同一个消费状态。

## 后果

- `EVT-03`、`EVT-04` 的 Symbol 身份改写为 Rust 类型身份；`EVT-19` 通过 `NAME` 验证聚合错误标签。`EVT-05` 的对象原型属性污染场景属于 JavaScript 对象模型，矩阵状态为“不适用”。
- `EVT-01`、`EVT-02` 的旧 `dispatch` 回调数组改写为派发快照，分别与 `EVT-11`、`EVT-14` 共同检验释放后的跳过及继续行为。
- `EVT-11`、`EVT-12` 的四种派发参数全部保留，分别使用两个事件族。
- 异步派发在方法调用时取得快照，返回任务由调用方轮询；确定性测试以明确的时钟推进覆盖并发等待。
