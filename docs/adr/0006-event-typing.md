# ADR-0006 事件的类型化

| 项 | 内容 |
|---|---|
| 状态 | 提议。实现者在里程碑 `M2` 确认或修订，修订时更新本文件并把状态改为“已接受” |
| 影响 | `DOC-02` §6、§7；`DOC-01` `S-EVT-*` |

## 问题

`UPSTREAM` 的事件以字符串或符号为名，监听器的签名通过 TypeScript 的 `Events` 接口声明。同一个事件可以用五种方式派发（`emit`、`parallel`、`serial`、`bail`、`waterfall`），监听器是同步还是异步由调用约定决定，运行时不区分。Rust 中同步回调与异步回调是不同的类型，需要决定事件如何标识，以及派发方式与监听器签名的关系。

## 选项

| 选项 | 内容 | 代价 |
|---|---|---|
| A | 字符串名，参数与返回值为 `Box<dyn Any>` | 类型错误推迟到运行期 |
| B | 事件是类型 `E: Event`；每个事件在声明时确定所属的派发族，监听器签名随之确定 | 一个事件不能同时以同步方式和异步方式派发 |
| C | 事件是类型；所有监听器统一为异步 | `emit`、`bail` 失去同步语义；`UPSTREAM` 的 `internal/get`、`internal/config` 等同步瀑布事件无法表达 |

## 判据

1. `DOC-00` `P5`：能交给类型系统的检查交给类型系统。
2. `DOC-01` `S-EVT-4`、`S-EVT-6`、`S-EVT-8`：`emit`、`bail`、`waterfall` 是同步的；内核事件 `internal/config`、`internal/listener`、`internal/update` 分别以 `waterfall`、`bail`、`waterfall` 派发，且在同步路径上使用。
3. `UPSTREAM` 用例对五种方式各有独立的断言，五种方式都要存在。

## 决定

选 B。派发族有三个。

| 派发族 | 监听器 | 可用的派发方法 |
|---|---|---|
| 同步通知 | `Fn(&E::Args) -> Option<E::Output>` | `emit`（忽略返回值）、`bail`（返回第一个 `Some`） |
| 异步通知 | `Fn(&E::Args) -> BoxFuture<Result<Option<E::Output>>>` | `parallel`（全部并发，聚合错误）、`serial`（依次等待，返回第一个 `Some`） |
| 瀑布 | `Fn(E::Args, Next<E>) -> E::Output` | `waterfall` |

- `UPSTREAM` 的“非空结果”（`DOC-01` `S-EVT-6`）对应 `Some`。
- 事件名取 `E::NAME`，用于 `internal/dispatch`、错误消息和效应标签 `ctx.on("name")`。
- 过滤器（`S-EVT-3`）是派发方法的可选参数，类型为 `&dyn Fn(&Context) -> bool`，实参是监听器的登记上下文。

## 后果

- 用例 `EVT-03`、`EVT-04`、`EVT-19`（符号事件名）与 `EVT-05`（原型属性名）检验的是 JS 的键类型，预期状态为“不适用”或“已改写”，由 `DOC-04` §3 的判定步骤确定。
- 用例 `EVT-01`、`EVT-02` 检验已弃用的 `dispatch` 方法，Rust 不提供该方法；它们断言的移除语义由 `EVT-11`、`EVT-14` 覆盖。
- `EVT-11`、`EVT-12` 是 `it.each(['emit', 'parallel', 'serial', 'bail'])`，Rust 侧对两个派发族各自的方法分别写测试。
