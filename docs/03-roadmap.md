# 03 路线图

| 项 | 内容 |
|---|---|
| 文档编号 | `DOC-03` |
| 状态 | 已定稿。里程碑的进展记录在 `docs/PROGRESS.md` |
| 读者 | 实现者、评审者 |
| 输入 | `DOC-01` 的条款、`DOC-02` 的模块划分与规则、`DOC-04` 的 177 条用例 |
| 输出 | 里程碑 `M0`–`M6`，每个里程碑的交付物与退出条件 |
| 不包含 | `DOC-00` §4 列出的范围外事项 |

## 1 推进方式

里程碑按编号顺序连续推进，上一个里程碑的退出条件全部满足后进入下一个，中间不等待评审。每个里程碑以一个 git commit 结束。工作因故中断时，下一次从 `docs/PROGRESS.md` 记录的位置继续。

里程碑的顺序由依赖决定：事件系统依赖效应；纤程生命周期依赖事件（内核事件）和效应；服务依赖纤程生命周期；拦截、日志、`replace` 依赖服务。

## 2 通用退出条件

每个里程碑结束时，下列命令在工作区根目录执行并全部成功。

| 编号 | 命令 | 成功的含义 |
|---|---|---|
| `X-1` | `cargo fmt --all --check` | 无格式差异 |
| `X-2` | `cargo clippy --workspace --all-targets -- -D warnings` | 无警告 |
| `X-3` | `cargo test --workspace` | 全部测试通过 |
| `X-4` | `conda run -n base python tools/conformance.py check --through M<n>` | 矩阵覆盖全部 177 条用例；属于 `M0`–`M<n>` 的用例都已离开“未开始”；已移植与已改写的行填有 Rust 测试位置；已改写与不适用的行填有理由 |
| `X-5` | `grep -rn "unsafe" crates/cordis/src` | 无输出（规则 `A-1`） |
| `X-6` | `find crates/cordis/src -name '*.rs' \| xargs wc -l` | 总行数不超过 6000（`G5`）；超过 4000 时在 `PROGRESS.md` 记录各模块行数 |

另有两项人工步骤：

- `X-7`：`docs/PROGRESS.md` 更新（格式见该文件）。
- `X-8`：一个 commit，消息格式 `M<n>: <摘要>`。

## 3 里程碑

### M0 工程骨架

| 项 | 内容 |
|---|---|
| 目的 | 建立可以承载后续全部里程碑的仓库与检查手段 |
| 交付物 | 根目录 `git init`；`.gitignore`（含 `/cordis/`、`/docs/reference/`、`/target/`）；`Cargo.toml` workspace；`rust-toolchain.toml`；`crates/cordis` 与 `crates/cordis-testkit` 两个空 crate，模块文件按 `DOC-02` §2 建好；`DOC-02` §9 白名单依赖的版本选定并记入 `PROGRESS.md` |
| 专有退出条件 | `X-1`–`X-3`、`X-5`–`X-8`；`conda run -n base python tools/conformance.py check` 通过（此时 177 条全部为“未开始”） |
| 用例 | 无 |

### M1 内核基座与同步效应

| 项 | 内容 |
|---|---|
| 目的 | 实现 `T-EFF-1`–`T-EFF-3`、`T-EFF-6`：可回退效应的同步部分，以及承载它的内核状态与上下文 |
| 模块 | `kernel.rs`、`context.rs`、`list.rs`、`effect.rs`（同步部分）、`fiber.rs`（仅根纤程）、`error.rs`；`cordis-testkit` 的 `TestExecutor` |
| 条款 | `T-EFF-1`、`T-EFF-2`、`T-EFF-3`、`T-EFF-6`；`S-CTX-1`–`S-CTX-4`；`S-EFF-1`（同步形态）、`S-EFF-2`、`S-EFF-3`、`S-EFF-4`、`S-EFF-6`、`S-EFF-7`；`S-FBR-3`、`S-FBR-17`；规则 `A-1`–`A-3` |
| 用例 | 4 条：`DSP-02`、`DSP-03`、`DSP-11`、`DSP-12` |
| 说明 | 这 4 条用例只在根上下文上调用 `effect`。另为 `DOC-02` 规则 `A-1`–`A-3`、视图链的 `extend`、`list.rs` 各写单元测试，它们没有对应的上游用例 |

### M2 事件

| 项 | 内容 |
|---|---|
| 目的 | 实现事件系统。后续里程碑的内核事件（`internal/*`）依赖它 |
| 模块 | `events.rs` |
| 条款 | `S-EVT-1`–`S-EVT-10`、`S-EVT-12`；规则 `A-4` |
| 用例 | 25 条：`events.spec.ts` 中里程碑为 `M2` 的行 |
| 说明 | 事件的类型化方式见 `ADR-0006`。重入用例（`EVT-09`–`EVT-12`、`EVT-24`–`EVT-27`）是规则 `A-3`、`A-4` 的主要检验 |

### M3 组件生命周期

| 项 | 内容 |
|---|---|
| 目的 | 实现 `T-LIF-*`：组件实例化、纪元、惯性状态机、异步效应、配置更新 |
| 模块 | `fiber.rs`、`registry.rs`（`mount`、`inject`、配置校验）、`effect.rs`（异步部分） |
| 条款 | `T-EFF-4`、`T-EFF-5`；`T-LIF-1`–`T-LIF-9`；`S-FBR-1`–`S-FBR-22`；`S-EFF-1`（异步形态）、`S-EFF-5`、`S-EFF-8`；`S-REG-1`–`S-REG-3`、`S-REG-6`、`S-REG-7`；`S-EVT-11`；规则 `A-5`–`A-10` |
| 用例 | 50 条：`component.spec.ts` 10 条、`fiber.spec.ts` 15 条、`update.spec.ts` 8 条、`dispose.spec.ts` 中 11 条（10 条异步用例与 `DSP-01`）、`events.spec.ts` 中 6 条依赖组件的用例 |
| 说明 | 本里程碑是内核的中心。`fiber.rs` 先实现 `DOC-02` §5.1 的调和循环，再接入其余功能。本里程碑内纪元只含修订号，依赖部分在 `M4` 接入。这三个 spec 文件中场景用到 `provide`、`inject` 或 `Service` 的 10 条用例归入 `M4`，清单见 `tools/conformance.py` 的 `OVERRIDES` |

### M4 反应式余效应

| 项 | 内容 |
|---|---|
| 目的 | 实现 `T-COE-1`–`T-COE-8`、`T-CTX-*`：服务的提供、访问中介、通知、依赖顺序、隔离 |
| 模块 | `reflect.rs`、`service.rs`（`ServiceKey`、`Scoped<K>` 的基本形态）、`context.rs`（`isolate`） |
| 条款 | `T-COE-1`–`T-COE-8`；`T-LIF-10`；`T-CTX-1`–`T-CTX-4`；`S-CTX-5`；`S-SVC-1`–`S-SVC-10`；`S-FBR-2`（通知部分）、`S-FBR-5`（依赖部分）、`S-FBR-12`（等待依赖方） |
| 用例 | 47 条：`service.spec.ts` 8 条、`service-dispose.spec.ts` 17 条、`reflect.spec.ts` 6 条、`isolate.spec.ts` 6 条，以及从 `M3` 归入的 10 条（`fiber.spec.ts` 7 条、`update.spec.ts` 2 条、`component.spec.ts` 1 条） |
| 说明 | `service-dispose.spec.ts` 的 17 条用例检验差异 `D-5`：提供方的卸载与绑定的撤回等待依赖方清理完毕。`FBR-01`–`FBR-03`（inertia lock）在本里程碑检验惯性状态机与依赖变化的交互 |

### M5 拦截、调用方句柄、日志、替换

| 项 | 内容 |
|---|---|
| 目的 | 实现 `T-COE-9`–`T-COE-13` 与内核的其余接口 |
| 模块 | `service.rs`（拦截、`Scoped<K>` 的完整形态）、`context.rs`（`intercept`）、`logger.rs`、`registry.rs`（`replace`） |
| 条款 | `T-COE-9`–`T-COE-13`；`S-SVC-11`–`S-SVC-13`；`S-LOG-1`–`S-LOG-5`；`S-REG-4`、`S-REG-5`；`S-FBR-4` |
| 用例 | 50 条：`intercept.spec.ts` 7 条、`shadow.spec.ts` 8 条、`associate.spec.ts` 6 条、`invoke.spec.ts` 2 条、`logger.spec.ts` 11 条、`replace.spec.ts` 16 条 |
| 说明 | `shadow`、`associate`、`invoke` 三组共 16 条用例依赖 `Proxy`。每条按 `DOC-04` §3 的判定步骤处理 |

### M6 收尾

| 项 | 内容 |
|---|---|
| 目的 | 验收目标 `G3`、`G4`，冻结接口 |
| 交付物 | `crates/cordis/examples/plugins.rs`（`DOC-02` §8）；多线程压力测试 `crates/cordis/tests/stress.rs`；`crates/cordis/README.md`；公开接口的文档注释；`docs/PROGRESS.md` 中的接口清单与遗留问题清单 |
| 用例 | 1 条：`DEC-01` |
| 专有退出条件 | `cargo run -p cordis --example plugins` 的输出符合 `DOC-02` §8 的五个步骤；压力测试在 tokio 多线程运行时上以至少 8 个工作线程运行 1000 轮，每轮结束后快照相等且无纤程停留在 `LOADING` 或 `UNLOADING`；`cargo doc --workspace --no-deps` 无警告；`tools/conformance.py check --through M6` 通过 |

## 4 用例分布

| 里程碑 | 用例数 | 累计 |
|---|---|---|
| `M1` | 4 | 4 |
| `M2` | 25 | 29 |
| `M3` | 50 | 79 |
| `M4` | 47 | 126 |
| `M5` | 50 | 176 |
| `M6` | 1 | 177 |

数字由 `conda run -n base python tools/conformance.py list` 的输出统计得到。

## 5 后续阶段

本次交付结束后，下列事项按需另行立项。它们都以组件的形式写在内核之上，不修改内核。

| 事项 | 对应 `PAPER` / `UPSTREAM` | 依赖内核的原语 |
|---|---|---|
| 声明式加载器 | §4.2.1；`packages/loader`、`include`、`group` | `mount`、`update`、`isolate`、`intercept`、`internal/commit`、名称登记表 |
| 热替换引擎 | §4.2.2；`packages/hmr` | `registry.replace` |
| 定时器、控制台日志等基础服务 | `packages/timer`、`logger-console` | `effect`、`provide`、日志导出器 |
| 类型化依赖声明的过程宏 | §5.3 | `Component`、`Inject` |
