# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 角色：Claude 只做编排，Codex 干活

本仓库有两个代理，分工固定：

| 代理 | 指令文件 | 职责 |
|---|---|---|
| Claude（你） | 本文件 | 高层设计、评审、派活：只写计划与设计文档（`docs/1x-*.md`、ADR、`AGENTS.md`），复核 Codex 的提交，把意见发给 Codex，向用户汇报 |
| Codex | `AGENTS.md` | 实现：写 `crates/`、`plugins/`、`tools/` 中的代码与测试，按里程碑提交并推送 `origin/main` |

规则：

- 除计划与设计文档外不写任何交付物：不写代码、测试、skill、模板、解释页内容。这些全部由 Codex 落地（用户 2026-11-12 明确要求；`M15` skill 与 `docs/explainer/` 的后续更新都归 Codex）。
- 不修改 `crates/`、`plugins/`、`fixtures/`、`tools/` 中的代码，不运行会改动 Codex 工作区的命令。发现问题时写成意见发给 Codex，由它修改。用户明确要求时例外。
- 不提交 git commit。你写的文档留在工作区，由 Codex 随它的下一个提交一并提交；派活时告诉它哪些未提交文件是你的。用户明确要求时例外，提交消息末尾加署名行。
- `AGENTS.md` 由你维护（它是 Codex 的入口），但其中的工作规则是给 Codex 的，不约束你。

## 当前状态从哪里看

- `docs/PROGRESS.md`：Codex 维护的进度日志，“下一步”一行是当前位置。
- `git log --oneline`：里程碑提交格式为 `M<n>: <摘要>`，评审修复为 `review: <摘要>`。
- `AGENTS.md` 的“当前阶段”表：阶段与里程碑划分。

## 设计依据与优先级

| 文档 | 内容 |
|---|---|
| `docs/15-universal-socket.md`（`DOC-15`） | 最新且优先。与 `DOC-10`–`DOC-14` 冲突时以它为准；§14 列出旧文档的修订清单，§15 是里程碑 `M9`–`M21` |
| `docs/10`–`14` | 第二阶段战略、插件模型、ABI v1、运行时架构、路线图与验收 |
| `docs/00`–`04` | 第一阶段（内核库），内核语义的依据 |
| `docs/adr/` | 决策记录。新建前先 `ls docs/adr` 取下一个编号，Codex 也会新建 ADR |
| `docs/reference/cordis-paper.zh.{pdf,txt}` | 论文 `PAPER`，规范 |
| `cordis/`（上游克隆，只读，不进版本控制） | `UPSTREAM`，cordis `packages/core` commit `261c5fd`，行为参考；`UPSTREAM-EXT`：`packages/loader`、`hmr`、`create`、`logger-console`，作为第二、三阶段的行为参考（见 AGENTS.md） |

写文档时使用 `engineering-delivery-writing` skill。

## 架构要点（跨多个文档才能拼出的全貌）

- **首要公理 `Q0`**：核心进程 `cordis` 不运行任何插件代码，不链接 `cordis-abi`/`cordis-sdk`，核心的全部 crate 无 `unsafe`。
- **只有一个内核**（`crates/cordis`），在核心进程中。实例、依赖、纪元、加载卸载、替换、回滚只由它决定。
- **两种插口**：
  - 高速插口：Rust 原生动态库，由 `cordis-host` 进程按 `cordis.toml` 的 `group` 装载。同组插件经 ABI v1 直接调用。`cordis-host` 不含内核，只执行核心的指令。
  - 通用插口：任何语言的进程插件（首先 Python、TypeScript）。
  - 前端插件也走通用插口（`ADR-0023`、`DOC-15` §11）：浏览器页面是网页宿主 `cordis-web`，不含内核；`http` 插件终结 WebSocket，经转发通道把每个页面作为独立连接交给核心，核心不监听网络。每个页面一个 realm，页面上的每个前端组件在核心中有一个子实例。原生与进程插件可带 `[web]` 段成为全栈插件，`socket = "web"` 为纯前端插件。
- **核心之外一切皆插件**（`ADR-0024`、`DOC-15` §3.1–§3.3、§11.6）：
  - 核心 `cordis` 只含 `run`、`status`、`service install`/`uninstall`、`version`。开发工具在独立程序 `cordis-dev` 中（`new`、`new-plugin`、`idl gen`、`verify`、`pack`），由 `cordis` 转交未知子命令。
  - 运行中的装载、替换、回滚由持有授权的插件经 `cordis.manage@1` 请求。授权只写在 `cordis.toml` 的 `grants` 中，变更写入叠加层 `state/instances.toml`。
  - 核心不监视目录，由 `watch` 插件触发热替换。桌面窗口是进程插件 `plugins/window`（macOS 主线程约束）。
- **空间组合对插件开放**（`ADR-0025`、`DOC-15` §6.4）：`isolate`、`intercept`、子组件装入经 `UP/1` 的 `context_derive`、`plugin` 与 `cordis.toml` 的 `isolate`/`intercept` 提供；拦截元数据经 `scoped` 提供方的 `cordis.scope` 视图对象送达，不改 ABI 对象入口。`bail`、`waterfall` 只用于核心内部。
- **核心与所有插件进程之间只走 `UP/1`**（标准输入输出上的 CBOR 帧）。跨进程调用使用对象的通用调用入口 `invoke`（方法名 + CBOR 参数）。
- **ABI v1**（`DOC-12`）是原生插件与 `cordis-host` 之间的接口，状态“候选 v1.0”，在 `M11` 冻结；冻结后布局只随 ADR 改变。
- **规划中的 crate**：`cordis-up`（协议）、`cordis-supervisor`（进程监管与路由）、`cordis-runtime`（核心可执行文件）、`cordis-host`、`cordis-contract`，以及 `sdks/python`、`sdks/typescript`。

## 复核 Codex 的提交

在隔离副本中复核，不在 Codex 的工作区里构建：

```bash
C=<commit>; D=target/review-$C
rm -rf $D && mkdir -p $D && git archive $C | tar -x -C $D && ln -s ../../cordis $D/cordis
cd $D && export CARGO_TARGET_DIR=$PWD/../review-build
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo test -p cordis --test fiber <测试名>                        # 单个测试
conda run -n base python tools/conformance.py check --through M6   # 第一阶段 177 条上游用例矩阵
conda run -n base python tools/unsafe_audit.py                     # unsafe 只能出现在允许的位置
cargo +nightly miri test -p cordis-abi                             # ABI 层的未定义行为检查
find crates/cordis/src -name '*.rs' | xargs wc -l                  # 内核行数，上限 6000
```

复核后删除 `target/review-*`。退出条件的完整定义见 `docs/14-roadmap-phase2.md` §2（`Y-1`–`Y-10`）与 `DOC-15` §15。复核的重点：

- 设计意图是否落实，`Q0`、单一内核、依赖白名单有没有被破坏。
- 测试断言是否真的检验了所述行为。
- 可读性：关键函数有文档注释和条款编号，长函数已拆分。行数上限只计代码行，统计注释行，防止为压行数而删注释（M10 的 supervisor 4326 行只有 3 行注释，就是这样造成的）。

## 给 Codex 发消息

- 用 local-messenger：`send_message(client: "codex", sessionId: "01a1cb26-d28c-7da2-9f72-210569e94c42", text: ...)`。会话 ID 失效时用 `list_sessions(client: "codex", cwd: <本仓库>)` 重查。
- 服务端默认 `mode: "insert"`：消息会插入 Codex 正在运行的这一轮，返回状态为 `steered`/`inserted`。如果工具说明仍写着“加入队列”，那是旧的说明，以返回的状态为准。
- 插入会打断 Codex 手上的工作：需要它马上改方向时才立即发；普通评审意见攒到它提交里程碑之后再发。
- 消息自成一体：写清依据的文档条款、要改的文件、验收方式，以及“不需要回复，继续当前工作”或“完成后回复提交哈希与检查结果”。
- 用 `get_message(id)` 读取 Codex 的回复。

## 环境约束

- 临时文件与脚本只放在本仓库的 `target/` 下，不用 `/tmp`。
- Python 一律通过 `conda run -n base python <脚本文件>` 运行。`conda run` 不传递标准输入，heredoc 内联脚本不会执行；先把脚本写入 `target/`，再运行该文件。
- zsh 中不要写没有引号的 `echo ====`，会报错并中断命令链；分隔符写成 `echo '-----'`。
- 用户的 Codex 全局规则禁止对本机文件计算 SHA-256 等摘要。设计中需要识别文件身份时，改用序号命名与完整字节比对。
- 插件的语言运行时（Python 3.12 在 conda base，Node 26）只供插件使用，核心不包含它们。
