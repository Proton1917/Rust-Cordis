# 进度日志

本文件记录实现的当前位置，供中断后继续。每个里程碑结束时更新；里程碑进行中发生需要后来者知道的事情时也更新。更新时改写“当前状态”一节，并在“里程碑记录”中补充对应小节，不在文件末尾堆叠流水账。

## 当前状态

| 项 | 值 |
|---|---|
| 最近完成的里程碑 | `M0` |
| 进行中的里程碑 | `M1` |
| 下一步 | 实现根上下文、同步效应、确定性执行器，移植 DSP-02、DSP-03、DSP-11、DSP-12 |
| 阻塞项 | 无 |
| 最后更新 | 2026-11-11，M0 验收 |

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

- commit：本节所在的 `M0: 建立工程骨架与固定工具链` 提交；使用提交主题定位，后继里程碑补记哈希。
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

## 决策与发现

实现过程中得到的、文档里没有的结论记在这里，每条写明日期、结论、依据（源码位置或用例编号）。需要改变 `DOC-01` 条款或 `DOC-02` 规则的结论另写 ADR，这里只留一行指向它。

- 2026-11-11：`DSP-03` 包含 `root.on`，其嵌套效应归属、元信息树与逆序释放属于 M1；事件表登记与移除在 M2 接入。依据 `dispose.spec.ts:37-74`。
