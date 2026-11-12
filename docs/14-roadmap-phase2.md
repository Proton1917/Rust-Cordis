# 14 第二阶段路线图

| 项 | 内容 |
|---|---|
| 文档编号 | `DOC-14` |
| 状态 | 已定稿（2026-11-11）。进展记录在 `docs/PROGRESS.md` |
| 读者 | 实现者、评审者 |
| 输入 | `DOC-10`–`DOC-13`、`ADR-0009`–`ADR-0014` |
| 输出 | 通用退出条件 `Y-1`–`Y-10`、里程碑 `M7`–`M14`、验收场景 `AC-1`–`AC-9`、skill 验收 `SK-1`–`SK-3`、完成定义 |
| 不包含 | `DOC-10` §4 的范围外事项 |

## 1 推进方式

与第一阶段相同：里程碑按编号连续推进，每个里程碑以一个提交结束，消息格式 `M<n>: <摘要>`，中间不等待评审。中断后从 `docs/PROGRESS.md` 的“下一步”继续。

顺序由依赖决定：SDK 依赖 ABI；装载器依赖 ABI 与内核改动；运行时依赖装载器；验证门依赖运行时的子进程；验收依赖全部。

## 2 通用退出条件

每个里程碑结束时，在工作区根目录执行下列检查并全部成功。

| 编号 | 检查 | 成功的含义 |
|---|---|---|
| `Y-1` | `cargo fmt --all --check` | 无格式差异 |
| `Y-2` | `cargo clippy --workspace --all-targets -- -D warnings` | 无警告 |
| `Y-3` | `cargo test --workspace` | 全部测试通过。测试需要的插件库由测试自身构建，不需要额外的手工步骤 |
| `Y-4` | `conda run -n base python tools/conformance.py check --through M6` | 第一阶段的一致性矩阵仍然成立 |
| `Y-5` | `conda run -n base python tools/unsafe_audit.py` | 内核无 `unsafe`；`unsafe` 只出现在 `B-2` 允许的位置，每处有 `// SAFETY:` 注释。脚本在 `M8` 编写 |
| `Y-6` | `cargo +nightly miri test -p cordis-abi` | `M8` 起执行。Miri 未报告未定义行为。nightly 版本记录在 `PROGRESS.md` |
| `Y-7` | `cargo test -p cordis-abi --test layout` | `M8` 起执行。生成的布局与 `docs/abi/v1-layout.txt` 一致 |
| `Y-8` | 统计各 crate 行数 | 不超过 `DOC-13` §1.1 的上限；超过目标时在 `PROGRESS.md` 记录 |
| `Y-9` | 人工 | `docs/PROGRESS.md` 更新：退出条件结果、决策与发现、遗留、下一步 |
| `Y-10` | 人工 | 一个提交，消息 `M<n>: <摘要>`，推送到 `origin/main` |

## 3 里程碑

### M7 内核：跨边界前提

| 项 | 内容 |
|---|---|
| 依赖 | 第一阶段完成；`K-5` 已提交 |
| 交付 | `DOC-13` §2 的 `K-1`–`K-4`；`ADR-0011` 的实现 |
| 测试 | 每项改动的验证用例（见 `DOC-13` §2 表格最后一列）；命名键与类型化键混用的服务替换、隔离域、拦截用例 |
| 退出 | `Y-1`–`Y-4`、`Y-8`–`Y-10` |

### M8 ABI v1

| 项 | 内容 |
|---|---|
| 依赖 | `M7` |
| 交付 | `crates/cordis-abi`；`tools/unsafe_audit.py`；`docs/abi/v1-layout.txt`；`DOC-12` 按实现定稿，状态改为“已冻结 v1.0” |
| 测试 | 每个类型的布局测试；同一进程内的双向往返：Rust 闭包 → `AbiObject` → 调用 → 释放；`AbiFuture` 的 Pending/Ready/取消；`AbiBuf` 跨“分配方”释放（用两个不同的分配函数模拟）；panic 转错误；虚表长度不足时返回 `code = 3`；租约计数在全部值释放后归零 |
| 退出 | `Y-1`–`Y-10` |

### M9 SDK 与宏

| 项 | 内容 |
|---|---|
| 依赖 | `M8` |
| 交付 | `crates/cordis-sdk`、`crates/cordis-sdk-macros`；接口 crate `plugins/kv-api` 与可独立编译的 `plugins/kv-mem`、`plugins/counter` 动态库 |
| 测试 | 回环宿主：在测试进程内实现一个最小 `HostV1`，把示例插件作为 rlib 链接，通过入口函数调用，覆盖 `DOC-12` §9 的每个宿主函数；`#[interface]` 的次版本追加（v1.0 使用方调用 v1.1 提供方、反向调用返回 `code = 3`）；`#[event]` 追加字段后旧监听器仍能解码 |
| 退出 | `Y-1`–`Y-10` |

### M10 装载器与热替换

| 项 | 内容 |
|---|---|
| 依赖 | `M7`、`M9` |
| 交付 | `crates/cordis-loader`；`fixtures/` 下的测试插件：同一插件名的 v1/v2、`apply` 失败版本、方法内 panic 版本、接口主版本不兼容版本、`panic = "abort"` 版本、ABI 版本不符版本 |
| 测试 | 真实 `dlopen` 装载 fixture；`DOC-11` §6 的每一步及其失败路径；§7 回滚；状态迁移往返；替换完成后旧库租约归零；`unload = "when-unleased"` 时租约归零后关闭库；替换过程中 8 个线程持续调用被替换的服务，不出现崩溃，调用要么由旧实现完成、要么返回错误、要么由新实现完成 |
| 退出 | `Y-1`–`Y-10` |

### M11 核心可执行文件

| 项 | 内容 |
|---|---|
| 依赖 | `M10` |
| 交付 | `crates/cordis-runtime`（可执行文件 `cordis`）；`plugins/console-log`、`plugins/kv-file`；示例部署目录 `examples/deploy/`（`cordis.toml`、构建脚本） |
| 测试 | 以子进程启动 `cordis run`：配置增删改与插件目录变化的每一行（`DOC-11` §10 表格）；`SIGTERM` 有序退出后重启，状态继承；配置文件写坏时保持运行；`status.json` 内容 |
| 退出 | `Y-1`–`Y-10` |

### M12 验证门

| 项 | 内容 |
|---|---|
| 依赖 | `M11` |
| 交付 | `crates/cordis-contract`；`cordis verify` 子命令；`plugins/kv-contract` |
| 测试 | `kv-mem` 与 `kv-file` 都通过 `kv-contract`，各自获得验证副本与印记；违反契约的 fixture 报告失败；`require_verified = true` 时缺少印记或与验证副本的完整字节比较失败的候选在 `SW-2` 被拒绝；被测库在套件运行中崩溃时，`cordis verify` 报告失败并正常退出 |
| 退出 | `Y-1`–`Y-10` |

### M13 验收

| 项 | 内容 |
|---|---|
| 依赖 | `M12` |
| 交付 | §4 的验收场景测试；`RM-1`–`RM-6` 的实测记录；根 `README.md` 的第二阶段章节（核心可执行文件的构建与运行、编写第一个插件）；`docs/PROGRESS.md` 的第二阶段总结 |
| 退出 | `Y-1`–`Y-10`；§4 全部场景通过 |

### M14 插件编写 skill

`M7`–`M13` 产出的规则分散在 `DOC-11`、`DOC-13` 和 ADR 中，读者是平台的实现者。今后的插件由人或 AI 编写和重写（`DOC-10` §1 第 4 条），他们需要一份面向插件作者的操作手册。`M14` 把它做成一个 skill，供 Claude Code 与 Codex 加载。

| 项 | 内容 |
|---|---|
| 依赖 | `M13`。SDK 与核心的使用方式在 `M13` 后不再变化，skill 以实际可运行的接口为准 |
| 执行者 | 设计方（Claude）编写；Codex 在 `M13` 完成后停止，不进入本里程碑 |
| 交付 | `skills/cordis-plugin/SKILL.md`：何时使用、插件项目结构、接口 crate、`Component` 实现、服务的提供与注入、事件、资源规则 `PR-1`–`PR-8` 的正确写法与错误写法、状态导出与继承、契约套件、`cordis verify`、编译与部署、常见错误及其症状；`skills/cordis-plugin/templates/`：可直接复制的接口 crate、插件 crate、契约套件 crate 模板；`skills/cordis-plugin/references/`：按需加载的接口版本规则与迁移指南（把既有 Rust 代码拆成插件的步骤） |
| 安装 | 同一份 skill 链接或复制到 Claude Code 与 Codex 的用户 skill 目录；仓库中的版本为唯一来源 |
| 验收 | `SK-1`：在没有本项目上下文的新会话中只加载该 skill，从空目录写出一个注入 `kv@1` 并导出状态的插件，编译后放入运行中的 `cordis`，插件进入 `Active`，热替换后状态保留。`SK-2`：同样条件下为 `kv@1` 写出第三个实现，通过 `kv-contract`。`SK-3`：skill 中的每段代码与模板都能在当前 SDK 上编译，由 `cargo test -p cordis-runtime --test skill_examples` 检查 |
| 退出 | `Y-1`–`Y-3`、`Y-9`、`Y-10`；`SK-1`–`SK-3` 通过 |

## 4 验收场景

场景在 `crates/cordis-runtime/tests/acceptance.rs` 中以子进程方式运行 `cordis`，使用示例插件与 fixture。

| 编号 | 场景 | 通过的判据 |
|---|---|---|
| `AC-1` | 以包含至少 8 个独立编译的插件库、合计 100 个实例的配置启动 `cordis` | 全部实例在 30 秒内进入 `Active`；`status.json` 列出每个库与实例 |
| `AC-2` | `counter` 持续写入时，在逻辑插件 `kv` 内把 `kv-mem` 实现替换为 `kv-file` 实现，再替换回 `kv-mem` 的新版本 | 每次替换后 `counter` 自动重建并继续计数，计数值不回退（状态迁移）；未注入 `kv@1` 的实例纪元不变；被替换库的租约在替换报告中归零 |
| `AC-3` | 替换为 `apply` 失败的版本 | 自动回滚；旧版本以原状态块恢复；`status.json` 标记“已回滚”；同一文件不被自动重试 |
| `AC-4` | 替换为不兼容接口主版本的版本 | 在 `SW-4` 被拒绝；任何实例的纪元不变 |
| `AC-5` | 发送 `SIGTERM` 后重启 | 全部实例的状态从 `state/` 继承；`state/consumed/` 收到对应文件 |
| `AC-6` | `require_verified = true`，分别放入已验证与未验证的新版本 | 已验证的被替换；未验证的被拒绝且运行状态不变 |
| `AC-7` | 服务方法内 panic；`apply` 内 panic | 调用方收到 `code = 5` 的错误，核心继续运行；`apply` panic 的实例进入 `Failed`，其余实例不受影响 |
| `AC-8` | 用 Rust 1.85.0（本机已安装，早于 `rust-toolchain.toml` 固定的 1.96.0）构建 `counter`，装载运行 | 与 1.96.0 编译的结果行为一致；`build_info` 显示两种编译器版本 |
| `AC-9` | 在 8 线程持续调用下，对同一插件连续替换 50 次 | 无死锁、无崩溃；记录 `RM-3`、`RM-4` 的中位数与最大值、`RM-6` |

## 5 完成定义

第二阶段在以下条件全部成立时完成：

1. `M7`–`M14` 的退出条件全部满足，`docs/PROGRESS.md` 有八个里程碑的记录。
2. `AC-1`–`AC-9` 全部通过。
3. `RM-1`–`RM-6` 有实测数值，记录在 `PROGRESS.md`。
4. `DOC-12` 状态为“已冻结 v1.0”，`docs/abi/v1-layout.txt` 与实现一致。
5. 按根 `README.md` 的“编写第一个插件”一节，从空目录新建一个插件 crate，编译后放入运行中的 `cordis` 的插件目录，插件被装载并运行。
