# 进度日志

本文件记录实现的当前位置，供中断后继续。每个里程碑结束时更新；里程碑进行中发生需要后来者知道的事情时也更新。更新时改写“当前状态”一节，并在“里程碑记录”中补充对应小节，不在文件末尾堆叠流水账。

## 当前状态

| 项 | 值 |
|---|---|
| 最近完成的里程碑 | `M4` |
| 进行中的里程碑 | `M5` |
| 下一步 | 集成 Scoped 拦截与定义点、日志服务及 registry.replace，验收 M5 的 50 条用例 |
| 阻塞项 | 无 |
| 最后更新 | 2026-11-12，M4 验收 |

## 环境

| 项 | 值 | 来源 |
|---|---|---|
| Rust 工具链 | 1.96.0 stable，`aarch64-apple-darwin` | 2026-11-11 `rustc --version` |
| `UPSTREAM` | `cordis/`，commit `261c5fdae6edb54e7ed4f071024f530acd18903b` | 2026-11-11 `git -C cordis log -1` |
| Python | `conda run -n base python`（仅用于 `tools/conformance.py`） | 项目约定 |
| 依赖版本 | slotmap 1.1.1；parking_lot 0.13.0；futures-core / futures-util 0.3.34；thiserror 2.0.21；serde 1.0.229；serde_json 1.0.154；tokio 1.54.1 | M0：逐项 `cargo search <crate> --limit 1`；workspace 固定版本，按模块调用引入 crate |
| 硬件 | Apple Silicon；14 个逻辑处理器；48 GiB 内存 | `sysctl -n hw.memsize hw.logicalcpu` |

## 里程碑记录

### M0 工程骨架

- commit：`c2c4f83`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：`cargo test --workspace` 通过，两个空 crate 和文档测试共 0 条；`cargo build` 通过。
- X-4：`conda run -n base python tools/conformance.py check` 通过，177 条均为初始状态。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 24 行。
- X-7：已记录环境、依赖版本及 M1 入口。
- X-8：提交主题 `M0: 建立工程骨架与固定工具链`。
- 矩阵：M0 分配 0 条用例。
- 偏离：无。
- 遗留：无。后续继续构建，保留本项目 `target/` 缓存。

### M1 内核基座与同步效应

- commit：`fe905f3`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：`cargo test --workspace` 通过，共 9 条测试；`cargo build` 通过。覆盖局部释放幂等、嵌套效应元信息、失败与 panic 后恢复、跨线程重入、根所有权、视图继承、有序移除及确定性时间。
- X-4：`conda run -n base python tools/conformance.py check --through M1` 通过：已移植 1、已改写 3、初始状态 173。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 722 行。
- X-7：已更新当前状态、矩阵及接口草图。
- X-8：提交主题 `M1: 实现上下文视图与可回退同步效应`。
- 矩阵：本里程碑已移植 1 条、已改写 3 条、不适用 0 条。
- 偏离：无架构规则偏离。`defer`、`adopt`、释放任务通过 `Result` 交回错误；按 DOC-02 §6 同步接口草图。
- 遗留：核心服务按 M2–M5 接入根上下文；异步效应、子纤程与惯性调和属于 M3。继续构建，保留 `target/`。

### M2 事件

- commit：`b258d75`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：`cargo test --workspace` 通过，共 32 条测试，事件测试 23 条；`cargo build` 通过。覆盖五种派发、四模式的删除与新增快照、一次性递归与并发调用、同步与异步瀑布、过滤、聚合错误、登记接管及派发观察。
- X-4：`conda run -n base python tools/conformance.py check --through M2` 通过：累计已移植 8、已改写 20、不适用 1、初始状态 148。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 1335 行。
- X-7：已更新当前状态、矩阵、接口草图及 ADR-0006。
- X-8：提交主题 `M2: 实现类型化事件与可回退监听器`。
- 矩阵：本里程碑已移植 7 条、已改写 17 条、不适用 1 条。
- 偏离：无架构规则偏离；ADR-0006 确认三种派发族及异步瀑布返回值。
- 遗留：纤程专属的 internal/update 行为随 M3 接入。继续构建，保留 `target/`。

### M3 组件生命周期

- commit：`7b96587`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：`cargo test --workspace` 通过，共 77 条单元及集成测试、2 条 compile_fail 文档测试；`cargo build` 通过。覆盖加载与卸载惯性、配置更新合并、失败恢复、丢弃等待句柄、步骤边界取代、异步逆序清理、嵌套组件、配置问题路径及状态监听器 panic 后继续调和。
- X-4：`conda run -n base python tools/conformance.py check --through M3` 通过：累计已移植 8、已改写 70、不适用 1、初始状态 98。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 2395 行。
- X-7：已更新矩阵、接口草图、Scope 租约说明及当前状态。
- X-8：提交主题 `M3: 实现惯性生命周期与异步效应`。
- 矩阵：本里程碑已改写 50 条；非法组件与非法效应值通过 compile_fail 文档测试检验公开接口的类型约束。
- 偏离：ADR-0008 规定 Fiber 持有所属 Scope，已失活记录随最后一个相关句柄回收。
- 遗留：依赖纪元、绑定可用性与依赖方等待在 M4 接入；内核错误当前集中记录，M5 接入日志服务。继续构建，保留 `target/`。

### M4 反应式余效应

- commit：本节所在的 `M4: 实现反应式服务与依赖清理顺序` 提交。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：首次 `cargo test --workspace` 通过，120 条运行测试与 2 条文档测试。随后用原始 CMP-10 触发复现调度差异，并以新增用例复现 InternalService panic 中断提供；修复后 `cargo test -p cordis --test component --test fiber --test update --test events_lifecycle --test reflect --test service --test service_dispose --test isolate` 的 83 条受影响测试通过，其余有效结果复用。当前共 121 条运行测试与 2 条文档测试；`cargo build` 通过。
- X-4：`conda run -n base python tools/conformance.py check --through M4` 通过：累计已移植 8、已改写 116、不适用 2、初始状态 51。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 3142 行。
- X-7：已更新矩阵、当前状态及 S-FBR-12 的调度步骤。
- X-8：提交主题 `M4: 实现反应式服务与依赖清理顺序`。
- 矩阵：本里程碑已改写 46 条、不适用 1 条。RFL-01 检验 JavaScript 子类的 Context.is 动态识别，Rust 使用 Context 类型及 Debug 映射，记录为不适用。
- 偏离：依据上游源码补正 S-FBR-12；架构规则继续沿用。
- 遗留：日志、拦截及组件替换在 M5 接入。继续构建，保留 `target/`。

## 决策与发现

实现过程中得到的、文档里没有的结论记在这里，每条写明日期、结论、依据（源码位置或用例编号）。需要改变 `DOC-01` 条款或 `DOC-02` 规则的结论另写 ADR，这里只留一行指向它。

- 2026-11-11：`DSP-03` 包含 `root.on`，其嵌套效应归属、元信息树与逆序释放属于 M1；事件表登记与移除在 M2 接入。依据 `dispose.spec.ts:37-74`。
- 2026-11-12：`CMP-10` 在挂载后立即释放根，仍执行一次子组件加载；卸载调用逆变换前的调度让步决定该顺序。已按 `fiber.ts:495-499` 补正 S-FBR-12 并恢复原始测试触发。
- 2026-11-12：内核派发的 InternalService 监听器 panic 按 A-9 记录，绑定建立与撤回继续完成；调用方直接 emit 的 panic 传播语义沿用 M2。
