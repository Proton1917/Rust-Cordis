# Rust-Cordis

论文《一种面向时空可组合性的编程范式》所定义运行时内核的 Rust 实现：可回退效应、反应式余效应与组件生命周期。上层程序以组件（插件）的形式写在内核之上。

> A small Rust kernel for spatiotemporally composable programs: reversible effects, reactive coeffects and component lifecycles. Programs are written as plugins on top of it. A Rust implementation of the [cordis](https://github.com/cordiverse/cordis) model.

## 定位

内核回答两个问题：一个组件占用过的东西，停用时如何完整归还；一个组件依赖的东西变化时，它如何被自动重建。

- 组件在激活时向上下文贡献效应（监听器、服务、子组件、任意资源）。每个效应登记自己的逆变换，归属于所在的纤程。组件停用、重载或失败时，内核按相反顺序回退全部效应。
- 组件声明自己需要哪些服务。服务出现时组件加载，服务被替换时组件重载，服务消失时组件卸载。提供方卸载之前，依赖方先完成清理。
- 加载与卸载期间到达的变更不打断在途的迁移。迁移结束后，纤程直接转向最新的目标状态。

内核只包含这套机制。加载器、热替换、定时器、日志输出等能力都以组件的形式写在内核之外。

## 两份依据

| 代号 | 对象 | 角色 |
|---|---|---|
| `PAPER` | 论文《一种面向时空可组合性的编程范式》§3、§4.1、§5.3 | 规范 |
| `UPSTREAM` | [cordiverse/cordis](https://github.com/cordiverse/cordis) `packages/core`，commit `261c5fd` | 参考实现；其 177 条 vitest 用例是行为依据 |

两者矛盾时以 `PAPER` 为准。`PAPER` 未覆盖的细节以 `UPSTREAM` 为准。每条上游用例在 `docs/04-conformance.md` 中有一行处置记录。

## 概念与接口

| 论文概念 | 上游（TypeScript） | 本仓库（Rust） |
|---|---|---|
| 可回退效应 | `ctx.effect(() => dispose)` | `ctx.effect(label, \|fx\| { fx.defer(..) })`、`ctx.effect_async` |
| 组件与纤程 | `ctx.plugin(fn, config)` | `ctx.mount_with(&def, config)`，返回 `Fiber` |
| 配置更新、重启、释放 | `fiber.update` / `restart` / `dispose` | 同名方法，返回可等待的任务 |
| 事件 | 字符串事件名，`emit` / `parallel` / `serial` / `bail` / `waterfall` | 以类型为键的事件，分同步、异步、瀑布三族 |
| 反应式余效应（服务） | `ctx.provide` / `ctx.foo` / `inject` | 类型化服务键 `ServiceKey`，`ctx.provide` / `ctx.get` / `Inject` |
| 隔离域 | `ctx.isolate(name)` | `ctx.isolate::<K>(realm)` |
| 拦截元数据 | `ctx.intercept(name, config)` | `ctx.intercept::<K>(meta)` |

完整的映射表在 `docs/02-architecture.md` §7。

## 示例

下面的程序展示可回退效应和组件的加载、更新、释放。完整源码位于 [`crates/cordis/examples/readme.rs`](crates/cordis/examples/readme.rs)，在工作区根目录执行 `cargo run -p cordis --example readme`。

```rust
use cordis::{ComponentDef, Context, Inject};
use cordis_testkit::TestExecutor;

fn main() -> cordis::Result {
    // 宿主提供任务派发与时钟；这里用 testkit 的确定性执行器。
    let host = TestExecutor::new();
    let root = Context::new(host.clone());

    // 可回退效应：效应体登记逆变换，释放时按相反顺序执行。
    let effect = root.effect("greeting", |fx| {
        println!("effect on");
        fx.defer(|| {
            println!("effect off");
            Ok(())
        })
    })?;
    host.finish(effect.dispose())?;

    // 组件：激活时贡献效应；重载或停用时由内核完整回退。
    let server = ComponentDef::new("server", Inject::new(), |ctx, port: u16| async move {
        println!("listen {port}");
        ctx.effect("listener", move |fx| {
            fx.defer(move || {
                println!("close {port}");
                Ok(())
            })
        })?;
        Ok(())
    });
    let fiber = root.mount_with(&server, 8080u16)?;
    host.finish(fiber.settled())?;
    host.finish(fiber.update(9090u16))?;
    host.finish(fiber.dispose())?;
    host.finish(root.fiber().dispose())?;
    Ok(())
}
```

输出：

```
effect on
effect off
listen 8080
close 8080
listen 9090
close 9090
```

## 设计约束

| 约束 | 内容 |
|---|---|
| 宿主无关 | 内核不依赖 tokio。任务派发与时间由宿主通过 `Spawn`、`Clock` 两个 trait 注入 |
| 线程安全 | 公开句柄均为 `Send + Sync`。内核状态由一把锁保护；持锁期间不调用用户代码、不轮询用户 `Future` |
| 协作式取代 | 过时的加载运行到检查点或结束，再由内核回退。内核不通过丢弃 `Future` 取消加载 |
| 显式清理 | 清理由效应的逆变换完成，不依赖 `Drop`。根上下文需要显式 `dispose` |
| 无 `unsafe` | 内核 crate 不使用 `unsafe` |
| 规模 | `crates/cordis/src` 目标 4000 行，上限 6000 行 |
| 依赖 | 运行时依赖取自白名单：`slotmap`、`parking_lot`、`futures-util`、`thiserror` |

各项约束的依据见 `docs/00-strategy.md` 与 `docs/adr/`。

## 仓库布局

```
crates/cordis/          内核
crates/cordis-testkit/  确定性执行器与可推进时钟，供测试使用
docs/                   设计文档、决策记录、一致性矩阵、进度日志
tools/conformance.py    一致性矩阵的生成与校验
AGENTS.md               实现者的工作规则
```

`cordis/`（上游克隆）与 `docs/reference/`（论文）是本地参考资料，不纳入版本控制。`tools/conformance.py` 需要 `cordis/` 存在。

## 构建与检查

工具链版本由 `rust-toolchain.toml` 固定为 Rust 1.96.0。

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
conda run -n base python tools/conformance.py check   # 校验一致性矩阵
```

## 进度

实现按里程碑 `M0`–`M6` 推进，每个里程碑一个提交。

| 里程碑 | 内容 |
|---|---|
| `M0` | 工程骨架 |
| `M1` | 内核基座与同步效应 |
| `M2` | 事件 |
| `M3` | 组件生命周期 |
| `M4` | 反应式余效应 |
| `M5` | 拦截、调用方句柄、日志、替换 |
| `M6` | 收尾：示例、多线程压力测试、接口冻结 |

`M0`–`M6` 及并发评审修复已提交，公开接口已完成 M6 验收。各里程碑的执行结果和接口清单记录在 `docs/PROGRESS.md`。

## 范围外

本仓库不包含声明式加载器、热替换引擎、动态代码装载、上游的周边包、过程宏、JS 绑定。前四项计划以组件的形式另行实现，见 `docs/03-roadmap.md` §5。

## 文档

| 文件 | 内容 |
|---|---|
| `docs/00-strategy.md` | 目标、范围、设计公理、冲突裁决顺序、风险 |
| `docs/01-semantics.md` | 从论文提炼的理论条款与从上游源码提炼的实现条款 |
| `docs/02-architecture.md` | 模块划分、数据模型、架构规则、接口草图、映射表 |
| `docs/03-roadmap.md` | 里程碑与退出条件 |
| `docs/04-conformance.md` | 177 条上游用例的处置矩阵 |
| `docs/adr/` | 决策记录 |
| `docs/PROGRESS.md` | 进度日志 |

## 许可证

MIT，与上游 cordis 一致，全文见 `LICENSE`。本仓库的设计与测试用例参照 [cordiverse/cordis](https://github.com/cordiverse/cordis)（MIT，Copyright (c) 2021-present Shigma），其版权声明保留在 `LICENSE` 中。
