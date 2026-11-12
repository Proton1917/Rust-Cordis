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

第二阶段的配置装配、文件变化、脚手架与终端日志，分别参考同一提交的 loader、hmr、create、logger-console。Rust 的批次回滚、进程分组和静态配置规则在 ADR-0031–0033 中说明。

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
| 规模 | 内核目标 4800 代码行，上限 6000；空行与注释分别统计，见 DOC-13 §1.1 |
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

原生验收会自动构建真实插件库，并使用 Rust 1.85.0 验证较旧编译器生成的插件。开发与运行工具可以一起构建：

```bash
cargo build --workspace --target-dir target
target/debug/cordis version
target/debug/cordis new target/hello-cordis --sdk-path "$PWD/crates/cordis-sdk"
cd target/hello-cordis
cargo build --target-dir target
../../target/debug/cordis run
```

生成的项目包含 app 与 watch。watch 合并一个防抖窗口内的构建产物和配置变化，核心统一检查并替换，失败恢复原配置、原版本及原状态。管理授权写在 `cordis.toml`，运行中变更保存在状态目录的叠加层；批次受理票据与实际结果分别记录，watch 自身重建后继续处理待确认窗口。

文件 KV、计数组件和终端日志的部署示例位于 [`examples/native`](examples/native/cordis.toml)：

```bash
bash examples/native/build.sh
cd examples/native
./cordis run
```

另一个终端可在该目录执行 `./cordis status` 读取实例与库状态。`SIGTERM` 或 `SIGINT` 触发最终状态导出和有序退出；再次运行时继承保存的状态。

## 契约验证

`cordis verify` 转交独立开发工具，启动隔离的核心和宿主执行真实套件。下面的命令验证内存 KV 的读写、键顺序、flush 和状态继承：

```bash
mkdir -p target/verify-demo
target/debug/cordis verify \
  --plugin target/debug/libkv_mem.dylib \
  --suite target/debug/libkv_contract.dylib \
  --project target/verify-demo
```

Linux 使用相应的 `.so` 文件名。验证成功后，命令保存已验证副本和库旁的 `.verified.json` 印记，逐项输出断言结果与验证时长。文件 KV 可通过 `--config` 指定包含 `path` 的 TOML 配置文件。

部署设置 `runtime.require_verified = true` 后，宿主在打开候选库前比较完整字节，握手后核对实际提供接口的契约覆盖。安装引用和宿主缓存库携带印记，有状态重启继续执行同一检查。

## 运行核心与编写第一个插件

在仓库根目录构建运行程序，并把它们所在目录加入当前终端的 PATH：

```bash
cargo build --release --target-dir target -p cordis-runtime -p cordis-host -p cordis-dev
export PATH="$PWD/target/release:$PATH"
```

`cordis` 负责运行和状态管理，`cordis-host` 在独立进程中装载原生插件，`cordis-dev` 执行开发命令。三者放在同一目录时，核心可以直接找到宿主和开发工具。

从新目录创建插件项目：

```bash
cordis new demo
cd demo
cordis new-plugin hello --rust
cargo build --target-dir target
cordis run
```

项目包含 `app`、`watch` 和新建的 `hello`。SDK 依赖固定到开发工具构建时的 Git 修订；在本仓库开发 SDK 时，可以在仓库根目录执行 `cordis new demo --sdk-path "$PWD/crates/cordis-sdk"`。构建命令中的 `--target-dir target` 使动态库落在项目配置使用的 `target/debug`，也适用于设置了全局 `CARGO_TARGET_DIR` 的环境。

另开一个终端，进入同一个 `demo` 目录执行 `cordis status`，可以在实例列表中查看 `hello` 的 `active` 状态。修改 `plugins/hello/src/lib.rs` 后再次执行 `cargo build --target-dir target`，`watch` 会把同一防抖窗口的变化作为一批提交给管理接口。批次失败时，运行配置、插件版本和原状态共同恢复。Ctrl-C 触发有序退出并保存检查点。

生成的 `hello` 组件在激活时发送日志事件；部署 `console-log` 插件可以把这些事件显示在终端。服务、效应、隔离域、状态导出和管理授权的接口见 [插件模型](docs/11-plugin-model.md) 与 [SDK 接口](docs/13-runtime-architecture.md#4-sdk-接口)。

## 验证插件与部署为服务

契约套件在独立核心中运行。下面的命令在仓库根目录构建并验证内存 KV 实现：

```bash
cargo build --release --target-dir target -p kv-mem -p kv-contract
cordis verify --plugin target/release/libkv_mem.dylib --suite target/release/libkv_contract.dylib
```

macOS 动态库扩展名为 `.dylib`，Linux 使用 `.so`。验证成功后，工具保存 `.cordis/verified/` 中的完整库副本，并在被测库旁写入 `.verified.json`。配置 `require_verified = true` 后，宿主在打开候选库前比较完整字节，再核对接口契约覆盖范围。

在部署项目目录中执行 `cordis service install --name com.example.cordis.demo`，会生成并安装当前平台的 launchd 或 systemd 配置。只查看配置时使用 `--dry-run`。核心意外退出后由服务管理器重新启动，插件按检查点恢复；状态文件和插件数据保存在项目的 `state/` 中。

## 进度

第一阶段 `M0`–`M6` 交付内核，第二阶段推进独立核心、插件宿主与开发工具，每个里程碑一个提交。

| 里程碑 | 内容 |
|---|---|
| `M0` | 工程骨架 |
| `M1` | 内核基座与同步效应 |
| `M2` | 事件 |
| `M3` | 组件生命周期 |
| `M4` | 反应式余效应 |
| `M5` | 拦截、调用方句柄、日志、替换 |
| `M6` | 收尾：示例、多线程压力测试、接口冻结 |
| `M7`–`M9` | 拥有型名称、ABI、Rust SDK 与通用调用 |
| `M10` | UP/1 与唯一核心的进程监管 |
| `M11` | 原生插件宿主、组内替换、隔离域与组合；ABI v1.0 冻结 |
| `M12` | 核心可执行文件、声明式部署、管理接口、watch 与脚手架 |
| `M13` | 独立进程中的契约验证与验证印记 |
| `M14` | 系统验收、替换指标与使用文档 |

`M0`–`M13` 及评审修复已提交。原生宿主、核心命令、配置调和、批次管理、检查点、watch、脚手架与契约验证已通过实际验收，ABI v1.0 已冻结。M12 的三个产品解析目标均完成固定 1800 秒 fuzz，正式源码与被测源码已逐字节比较。当前推进 M14 系统验收与性能度量；各里程碑的结果和接口清单记录在 `docs/PROGRESS.md`。

## 后续阶段

第二阶段交付 Rust 原生插件、通用协议、契约验证与插件编写 skill。第三阶段按 `docs/15-universal-socket.md` 继续发布打包、WIT、Python 与 TypeScript SDK、浏览器前端插件和全栈插件，验收推进至 M21。

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
