# Rust-cordis

本仓库实现论文《一种面向时空可组合性的编程范式》所定义的运行时内核的 Rust 版本。内核很小，是今后各项目的核心：上层程序都以组件（插件）的形式写在它之上。组件声明自己需要哪些服务，激活时向上下文贡献效应，停用时这些效应被完整回退。

## 两份参考

| 代号 | 对象 | 位置 | 角色 |
|---|---|---|---|
| `PAPER` | 论文中文版 | `docs/reference/cordis-paper.zh.pdf`；可检索文本 `docs/reference/cordis-paper.zh.txt` | 规范 |
| `UPSTREAM` | cordis `packages/core`，commit `261c5fd` | `cordis/packages/core/`（`src` 2057 行，`tests` 4227 行） | 参考实现与行为用例 |

两者矛盾时以 `PAPER` 为准并写 ADR。`PAPER` 未覆盖的细节以 `UPSTREAM` 为准。

第二、三阶段另以同一上游提交中的以下包作为行为参考（`UPSTREAM-EXT`），只参考行为与配置语义，不移植代码，与 `Q0`、`DOC-15` 冲突时以后者为准：

| 包 | 位置 | 对应 | 参考内容 |
|---|---|---|---|
| `@cordisjs/loader` | `cordis/packages/loader/src` | `M12` `cordis.toml` 装配 | 配置条目的结构、分组、`isolate`、`disabled`，配置变化时的增量调和 |
| `@cordisjs/plugin-hmr` | `cordis/packages/hmr/src` | `M12` `watch` 插件 | 事务式热替换：一批文件变化作为一次替换，失败整体回滚 |
| `create-cordis` | `cordis/packages/create/src` | `cordis-dev new` | 项目脚手架流程 |
| `@cordisjs/logger-console` | `cordis/packages/logger-console/src` | `console-log` 插件 | 终端日志格式 |

本仓库由两个代理协作：设计方（Claude，指令在 `CLAUDE.md`）负责设计文档、ADR 与评审；你负责实现。`CLAUDE.md` 属于设计方，不需要阅读或修改；设计方写在工作区的未提交文档，随你的下一个提交一并提交。

## 当前阶段

| 阶段 | 里程碑 | 内容 | 状态 |
|---|---|---|---|
| 第一阶段 | `M0`–`M6` | 内核库 `crates/cordis` | 已完成（`e459aa7`，评审修复至 `940650c`） |
| 第二阶段 | `M7`–`M15` | 核心可执行文件 `cordis`、插件 ABI v1、Rust SDK、`UP/1` 与核心监管、插件宿主 `cordis-host`、验证门、插件编写 skill | 仓库交付与验收通过；Codex 用户 skill 链接已按授权安装 |
| 第三阶段 | `M16`–`M21` | 发布打包、WIT 接口描述语言、Python 与 TypeScript SDK、全栈插件 | M16–M18 与 AU-17 界面验收通过；接续 M19 |

全部里程碑由你实现，包括 `M15` 的 skill、`M21` 的 skill 章节，以及 `docs/explainer/` 解释页随进度的更新。里程碑之间不等待评审，从 `M12` 连续推进到 `M21`。

目标：核心编译成独立可执行文件，是系统中最稳定、长期不动的部分；核心进程不运行任何插件代码（`Q0`）。Rust 原生插件由 `cordis-host` 进程按分组装载（高速插口），任何语言的插件以进程形式接入（通用插口），前端插件在浏览器页面中经 `http` 插件转发的 WebSocket 接入（通用插口的第二种传输，浏览器中无内核），核心与所有插件之间只通过 `UP/1` 通信。核心在运行中装载、替换插件，依赖方自动重建。

`docs/15-universal-socket.md`（`DOC-15`）是最新且优先的设计依据：它与 `DOC-10`–`DOC-14` 不一致之处以 `DOC-15` 为准，并按其 §14 在对应里程碑中修订旧文档。文档按下列顺序阅读，第一阶段的文档（下一节）作为内核语义的依据继续有效。

| 顺序 | 文件 | 内容 |
|---|---|---|
| 1 | `docs/15-universal-socket.md` | 公理 `Q0`、进程架构、核心与开发工具 `cordis-dev` 分离、管理接口 `cordis.manage@1`、两种插口与分组、核心保护 `CP-1`–`CP-11`、核心崩溃恢复、ABI 修订 `V-1`–`V-4`、WIT、`UP/1` 要点、SDK、前端插件与全栈插件、脚手架与打包、文档修订清单、里程碑 `M9`–`M21`、验收 `AU-1`–`AU-18` |
| 2 | `docs/10-evolution-strategy.md` | 演化周期、目标 `G6`–`G13`、公理 `Q1`–`Q9`（`Q8` 已由 `Q0` 取代）、可替换性指标、风险 |
| 3 | `docs/11-plugin-model.md` | 插件的四个组成部分、接口版本、资源规则 `PR-*`、状态迁移、热替换步骤 `SW-*`、回滚、验证门、`cordis.toml` |
| 4 | `docs/12-abi-v1.md` | 原生插件与 `cordis-host` 之间的二进制接口（候选 v1.0，`M11` 冻结） |
| 5 | `docs/13-runtime-architecture.md` | crate 划分与规模上限、内核改动 `K-1`–`K-5`、SDK 草图、架构规则 `B-1`–`B-10`、依赖白名单 |
| 6 | `docs/adr/0009`–`0025` | 插件格式、ABI 与 CBOR、命名键、卸载语义、状态迁移、版本、ABI 所有权、依赖可用性、运行时接口、核心隔离、两种插口、WIT、进程控制与模糊测试、通用 ABI 预算、前端插件、核心之外一切皆插件、隔离域与拦截跨进程 |
| 7 | `docs/14-roadmap-phase2.md` | 退出条件 `Y-1`–`Y-10`、验收场景 `AC-1`–`AC-9`、skill 验收 `SK-1`–`SK-3`；里程碑编号以 `DOC-15` §15 为准 |

工作循环与第一阶段相同，退出条件改用 `Y-1`–`Y-10`，不涉及上游用例矩阵的步骤（工作循环第 2、5 步中关于矩阵的部分）跳过。两条要求冲突时，`Q0` 优先，其次按 `docs/10-evolution-strategy.md` §8 裁决。

对下方硬性规则的调整：

- 规则 2：核心进程的全部 crate（内核、`cordis-up`、`cordis-supervisor`、`cordis-runtime`）不使用 `unsafe`，不依赖 `cordis-abi` 与 `cordis-sdk`。`unsafe` 只允许出现在 `cordis-abi`、`cordis-host` 与宏生成的固定代码形态中，按 `DOC-13` 规则 `B-2` 管理。
- 规则 5 只约束内核 crate。其他 crate 的依赖取自 `DOC-13` §7 与 `ADR-0017`。
- 规则 7 中的加载器、热替换引擎、动态代码装载、过程宏移入范围；其余事项与 `DOC-15` §13 的后续事项仍在范围外。
- `docs/abi/v1-layout.txt` 在 `M11` 冻结前的修改记入 `PROGRESS.md`；冻结后只随 ADR 修改。
- 规则 6 与 `DOC-13` §1.1 各 crate 的行数目标和上限只计代码行，不计空行、注释与文档注释；`PROGRESS.md` 报告“代码行 / 总行”。行数只能靠重构降低（去重、复用内核能力、按职责拆分），不得删注释或压缩写法；重构后仍超上限时写 ADR 说明，继续工作，由设计方复核。
- 全部 crate 的模块与关键函数写文档注释：负责的步骤、前置条件、失败处理，并引用条款编号（`S-*`、`SW-*`、`CP-*`、`DOC-15` 章节、ADR）。超过约 60 行的函数按步骤拆分。

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
11. 编译目录共用（用户 2026-11-12 要求；起因是 `target/` 已有 267 个子目录、68G，同一批依赖被重复编译了几十次）：
    - 共用目录的位置是 `<编译根>/shared`。编译根取 `CARGO_TARGET_DIR`；未设置时取工作区的 `target/`。设计方复核时会把 `CARGO_TARGET_DIR` 指向别处，这样共用目录也随之移走，复核副本仍然隔离。
    - 测试夹具、候选工程、测试中调用的 `cargo build`、基准和测量，一律通过 `--target-dir` 指向这个共用目录。路径由一个公共的测试辅助函数给出，不在各个测试里写死 `target/<名称>`。
    - 主会话和每个子代理各用一个编译根，最多 3 个，避免互相争用 Cargo 的构建锁。新建候选不再使用独立的编译目录。
    - 唯一的例外是“生成的用户项目”（`cordis new` 产生的项目），产品行为要求它使用自己的 `target/`。这类测试通过后，删除该项目的编译缓存，只保留日志和证据。
    - 证据目录（日志、JSON、截图）与编译缓存分开存放。进度日志只引用证据路径，不引用编译缓存中的文件。
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

上下文被压缩之后，先重读本文件和 `docs/PROGRESS.md` 的“当前状态”，再继续工作。设计方通过消息交给你的事项，收到时就写进 PROGRESS 的“下一步”或“阻塞项”。这样压缩之后也不会遗漏。

并行（用户 2026-11-12 要求）：
- 同时保持 2 个子代理，模型 astra，思考深度 high。主会话的深度不变。
- 子代理负责互不重叠的部分，例如下一个里程碑的候选准备、独立的测试或测量、专项排障。
- 主会话负责整合进正式工作区、执行退出检查并提交。子代理不提交，也不修改主会话正在改的文件。

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
