# Rust-cordis

本仓库实现论文《一种面向时空可组合性的编程范式》所定义的运行时内核的 Rust 版本。内核很小，是今后各项目的核心：上层程序都以组件（插件）的形式写在它之上。组件声明自己需要哪些服务，激活时向上下文贡献效应，停用时这些效应被完整回退。

## 两份参考

| 代号 | 对象 | 位置 | 角色 |
|---|---|---|---|
| `PAPER` | 论文中文版 | `docs/reference/cordis-paper.zh.pdf`；可检索文本 `docs/reference/cordis-paper.zh.txt` | 规范 |
| `UPSTREAM` | cordis `packages/core`，commit `261c5fd` | `cordis/packages/core/`（`src` 2057 行，`tests` 4227 行） | 参考实现与行为用例 |

两者矛盾时以 `PAPER` 为准并写 ADR。`PAPER` 未覆盖的细节以 `UPSTREAM` 为准。

## 当前阶段

| 阶段 | 里程碑 | 内容 | 状态 |
|---|---|---|---|
| 第一阶段 | `M0`–`M6` | 内核库 `crates/cordis` | 已完成（`e459aa7`，评审修复至 `940650c`） |
| 第二阶段 | `M7`–`M13` | 核心可执行文件 `cordis`、插件 ABI v1、SDK、装载器与热替换、验证门 | 进行中，从 `M7` 开始 |

第二阶段的目标：核心编译成独立可执行文件，插件单独编译成动态库，核心在运行中装载、替换插件，依赖方自动重建。第二阶段的文档按下列顺序阅读，第一阶段的文档（下一节）作为内核语义的依据继续有效。

| 顺序 | 文件 | 内容 |
|---|---|---|
| 1 | `docs/10-evolution-strategy.md` | 系统形态、演化周期、目标 `G6`–`G13`、公理 `Q1`–`Q9`、可替换性指标、范围外、风险 |
| 2 | `docs/11-plugin-model.md` | 插件的四个组成部分、接口版本、资源规则 `PR-*`、状态迁移、热替换步骤 `SW-*`、回滚、验证门、`cordis.toml` |
| 3 | `docs/12-abi-v1.md` | 二进制接口规范（`M8` 冻结） |
| 4 | `docs/13-runtime-architecture.md` | crate 划分与规模上限、内核改动 `K-1`–`K-5`、SDK 草图、架构规则 `B-1`–`B-10`、依赖白名单 |
| 5 | `docs/adr/0009`–`0014` | 插件格式、ABI 规定者与 CBOR、命名键、卸载语义、状态迁移、版本 |
| 6 | `docs/14-roadmap-phase2.md` | 退出条件 `Y-1`–`Y-10`、里程碑 `M7`–`M13`、验收场景 `AC-1`–`AC-9`、完成定义 |

第二阶段的工作循环与第一阶段相同，退出条件改用 `Y-1`–`Y-10`，不涉及上游用例矩阵的步骤（工作循环第 2、5 步中关于矩阵的部分）跳过。两条要求冲突时按 `docs/10-evolution-strategy.md` §8 裁决。

第二阶段对下方硬性规则的调整：

- 规则 2 只约束内核 crate。`unsafe` 允许出现在 `cordis-abi`、`cordis-loader` 与宏生成的固定代码形态中，按 `DOC-13` 规则 `B-2` 管理。
- 规则 5 只约束内核 crate。其他 crate 的依赖取自 `DOC-13` §7。
- 规则 7 中的加载器、热替换引擎、动态代码装载、过程宏移入第二阶段范围；其余事项与 `DOC-10` §4 的事项仍在范围外。
- `docs/abi/v1-layout.txt` 在 `M8` 冻结后只随 ADR 修改。

## 阅读顺序

开始工作前按顺序读完下列文件。

| 顺序 | 文件 | 内容 |
|---|---|---|
| 1 | `docs/00-strategy.md` | 目标 `G1`–`G5`、范围外事项、设计公理 `P1`–`P10`、冲突裁决顺序、风险 |
| 2 | `PAPER` §3、§4.1、§5.3、附录 A | 模型的定义、核心库的六个算法、Rust 的可行性讨论、术语表 |
| 3 | `docs/01-semantics.md` | 理论条款 `T-*`、实现条款 `S-*`（带上游源码行号）、两者的差异 |
| 4 | `docs/02-architecture.md` | 仓库布局、模块划分、数据模型、架构规则 `A-1`–`A-12`、接口草图、映射表、依赖白名单 |
| 5 | `docs/adr/0001`–`0006` | 六项决策的选项、判据与后果 |
| 6 | `docs/03-roadmap.md` | 里程碑 `M0`–`M6` 与退出条件 |
| 7 | `docs/04-conformance.md` | 177 条上游用例的处置矩阵与判定步骤 |
| 8 | `docs/PROGRESS.md` | 当前位置 |

实现某个模块时，对照阅读它在 `UPSTREAM` 中对应的源文件与 spec 文件（对应关系见 `docs/02-architecture.md` §2）。

## 硬性规则

1. `cordis/` 是只读参考，其中任何文件不得修改，不纳入本仓库的版本控制。
2. 内核 crate 不使用 `unsafe`。
3. 持有内核锁期间不调用用户代码、不轮询用户 `Future`、不 `await`（规则 `A-3`）。
4. 不通过丢弃 `Future` 取消组件的加载（规则 `A-7`，`ADR-0005`）。
5. 内核 crate 的运行时依赖只取自 `docs/02-architecture.md` §9 的白名单。内核 crate 不依赖 tokio。
6. `crates/cordis/src` 总行数不超过 6000，目标 4000。
7. 不实现范围外事项（`docs/00-strategy.md` §4）：加载器、热替换引擎、动态代码装载、周边包、过程宏、JS 绑定、发布。
8. `docs/04-conformance.md` 的 ID、上游位置、上游用例三列由工具生成，不得手工修改。
9. 所有文件写在本工作区目录树内。临时文件放在 `target/` 或工作区内的临时子目录，不使用 `/tmp`。
10. Python 只通过 `conda run -n base python` 调用。

## 工作循环

对每个里程碑执行以下步骤。

1. 读 `docs/03-roadmap.md` 中该里程碑的条款清单与用例清单。
2. 对清单中的每条用例：读上游用例源码，按 `docs/04-conformance.md` §3 的判定步骤确定处置，写 Rust 测试。
3. 实现模块，使测试通过。实现时把相关条款编号写在关键函数的文档注释里，例如 `/// S-FBR-6`。
4. 执行 `docs/03-roadmap.md` §2 的退出条件 `X-1`–`X-6`，全部成功。
5. 更新 `docs/04-conformance.md` 中该里程碑的行，更新 `docs/PROGRESS.md`。
6. 提交一个 commit，消息格式 `M<n>: <摘要>`。
7. 进入下一个里程碑，不等待评审。

里程碑按 `M0` 到 `M6` 的顺序连续推进。工作中断后，从 `docs/PROGRESS.md` 的“下一步”继续。

## 遇到分歧时

| 情形 | 处理 |
|---|---|
| `UPSTREAM` 的行为与 `docs/01-semantics.md` 的条款不一致 | 以上游源码和用例为准，修正条款并在 `PROGRESS.md` 的“决策与发现”记一行 |
| `UPSTREAM` 的行为与 `PAPER` 不一致，且不在 `docs/01-semantics.md` 第三部分的差异表中 | 写 ADR，在差异表补一行，采纳 `UPSTREAM` 的做法，除非它违反 `PAPER` 的结构性保证 |
| 需要偏离架构规则 `A-*` 或映射表的语义 | 先写 ADR（问题、选项、判据、决定、后果），再实现 |
| 公开接口（`docs/02-architecture.md` §6）的签名需要调整 | 直接调整并同步更新接口文档；涉及 §7 映射表语义时按架构规则处理 |
| 一条用例无法判定处置 | 状态暂留“未开始”，在 `PROGRESS.md` 的“遗留”中写明卡在哪一步，继续其余用例 |
| 两条要求冲突 | 按 `docs/00-strategy.md` §6 的顺序裁决 |

ADR 文件命名为 `docs/adr/<四位编号>-<短名>.md`，格式参照已有的六份。

## 常用命令

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
conda run -n base python tools/conformance.py list              # 上游用例清单
conda run -n base python tools/conformance.py check --through M3   # 校验矩阵
grep -rn "unsafe" crates/cordis/src                             # 应无输出
find crates/cordis/src -name '*.rs' | xargs wc -l               # 内核行数
```

## 完成定义

第二阶段的完成定义见 `docs/14-roadmap-phase2.md` §5。第一阶段在以下条件全部成立时完成（已满足）：

1. `M0`–`M6` 的退出条件全部满足，`docs/PROGRESS.md` 有七个里程碑的记录。
2. `conda run -n base python tools/conformance.py check --through M6` 通过，177 条用例无一为“未开始”。
3. `cargo run -p cordis --example plugins` 的输出符合 `docs/02-architecture.md` §8 的五个步骤。
4. `docs/01-semantics.md` 的每条 `T-*` 条款在测试代码的注释中至少被引用一次。
