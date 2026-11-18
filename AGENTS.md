# Rust-cordis

论文《一种面向时空可组合性的编程范式》所定义运行时内核的 Rust 实现。核心进程 `cordis` 不运行插件代码；Rust 原生插件由 `cordis-host` 按分组装载，其他语言的插件以进程接入，前端插件经 `http` 插件接入；核心与插件之间只走 `UP/1`。

当前发布 v0.2.5。当前状态与下一步见 [PROGRESS](docs/PROGRESS.md)，2026-11-16 以前的完整记录见 [docs/history](docs/history/progress-2026-11-16.md)。

## 设计依据

| 文件 | 内容 |
|---|---|
| `docs/15-universal-socket.md`（DOC-15） | 最新且优先；与 DOC-10–14 冲突时以它为准 |
| `docs/11`、`12`、`13`、`16` | 插件模型与资源规则 `PR-*`、ABI v1、crate 划分与依赖白名单、UP/1 协议 |
| `docs/00`–`04` | 第一阶段：内核语义、架构规则 `A-*`、177 条上游用例矩阵 |
| `docs/adr/` | 决策记录，新建前 `ls docs/adr` 取下一个编号 |
| `docs/reference/cordis-paper.zh.pdf`（PAPER）、`cordis/`（UPSTREAM，只读） | 规范与行为参考；两者矛盾以 PAPER 为准 |

## 第一性原理

动手前先回答：这项职责的事实由谁唯一掌握？答案决定它放在哪里。

1. `Q0`：核心进程不运行任何插件代码。
2. 只有一个内核（`crates/cordis`），实例、依赖、生命周期、替换与回滚只由它决定。
3. 核心之外一切皆插件。给核心增加职责前，先证明只有核心掌握相关事实。
4. 协议与领域语义归终结它的插件，核心只转交不透明数据。
5. 先复用已有机制，不为单个使用方开特例；为某个应用提出的需求按通用能力设计。
6. 机密与业务载荷由插件读取和保存；核心不解析它们，也不写进状态、日志、诊断与错误信息。

## 硬性规则

1. `cordis/` 只读，不纳入版本控制。
2. 核心进程的 crate（`cordis`、`cordis-up`、`cordis-supervisor`、`cordis-runtime`）不使用 `unsafe`，不依赖 `cordis-abi`、`cordis-sdk`；`unsafe` 只出现在 `cordis-abi`、`cordis-host` 与宏生成代码中。
3. 内核 crate 不依赖 tokio，运行时依赖只取 `docs/02-architecture.md` §9 白名单；其他 crate 新增依赖要写 ADR（DOC-13 §7）。
4. 持有内核锁时不调用用户代码、不 `await`（`A-3`）；不通过丢弃 Future 取消组件加载（`A-7`）。
5. `crates/cordis/src` 代码行不超过 6000。
6. `docs/04-conformance.md` 的 ID、上游位置、上游用例三列由工具生成。
7. 文件只写在本工作区内，临时文件放 `target/`；Python 只用 `conda run -n base python`。
8. 测试夹具与测试里的 `cargo build` 用 `<编译根>/shared` 作为 target 目录（`cordis_testkit::build::shared_target_dir`）；测试现场由 `TestGuard` 管理。

## 交付规则

1. **一个改动一个提交。** 关联的实现和说明共同提交，消息写明目的、实际变化与已有验证结果。测试数量取自实际执行的用例。
2. **ADR 只用于这几类改动：** ABI、UP/1、描述符或持久状态格式变化；新增运行时依赖；改变核心与插件的职责边界；对外接口的语义变化。修 bug、重构、补测试、改文档不写 ADR。
3. **按改动选择验证。** 纯文案、设计和项目规则改动，通读受影响文字、核对事实与引用即可；页面排版或交互有变化时查看对应画面。Rust 源码或构建配置变化时构建受影响目标，按实际影响选择格式、Clippy 和行为测试。跨工作区的影响或用户明确要求全量验证时执行工作区测试。ABI 实现变化按影响执行 Miri 与 unsafe 审计，内核语义变化核对相应的上游用例。提交和阶段切换复用已有有效结果。
4. **发布只在用户要求时进行。** 发布前对每个资产按完整字节检查本机路径、用户名和私有项目名（`grep -c -a -F`，结果须为 0），下载后实际启动一次。结果写在 GitHub Release 说明里，仓库不新增记录文件，也不做单独的记录提交。
5. **不新增 `docs/measurements/` 文件**，除非某份设计文档要引用这项测量。
6. **PROGRESS 只写当前状态与下一步，保持一屏。** 做完的事项直接删掉，历史以 git log 为准。
7. 缓存清理不记录，也不单独提交。

## 程序实现的验证命令

下列命令供涉及相应程序行为的改动选用。

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
conda run -n base python tools/conformance.py check --through M6   # 177 条上游用例矩阵
conda run -n base python tools/unsafe_audit.py
MIRI_SYSROOT=$PWD/target/miri-sysroot TMPDIR=$PWD/target/tmp CARGO_TARGET_DIR=$PWD/target/review-build/miri \
  cargo +nightly miri test -p cordis-abi
conda run -n base python tools/source_lines.py                      # 各 crate 代码行 / 总行
```
