# 14 第二阶段路线图

| 项 | 内容 |
|---|---|
| 文档编号 | `DOC-14` |
| 状态 | 已定稿（2026-11-11）。进展记录在 `docs/PROGRESS.md` |
| 读者 | 实现者、评审者 |
| 输入 | `DOC-10`–`DOC-13`、`ADR-0009`–`ADR-0014` |
| 输出 | 通用退出条件 `Y-1`–`Y-10`、第二阶段里程碑 `M7`–`M15`、验收场景 `AC-1`–`AC-9`、skill 验收 `SK-1`–`SK-3`、完成定义 |
| 不包含 | `DOC-10` §4 的范围外事项 |

## 1 推进方式

与第一阶段相同：里程碑按编号连续推进，每个里程碑以一个提交结束，消息格式 `M<n>: <摘要>`，中间不等待评审。中断后从 `docs/PROGRESS.md` 的“下一步”继续。

顺序由依赖决定：SDK 依赖 ABI；UP/1 与核心监管提供唯一内核的进程控制；插件宿主把原生 ABI 接入 UP/1；运行时提供配置与部署；验证门复用独立进程；M14 验收完整系统。Codex 连续推进 M7–M15，随后按 DOC-15 §15 进入第三阶段。

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
| 交付 | SDK、宏、kv-api、kv-mem、counter；DOC-15 V-1–V-4 与 ABI 候选布局 |
| 测试 | 回环宿主通过真实入口覆盖全部 Host 槽；接口方法追加与事件字段兼容；同一接口类型化和通用调用结果一致；CBOR 资源的参数、返回、取消与释放；未知方法 code 3 |
| 退出 | `Y-1`–`Y-10` |

### M10 UP/1 与核心监管

| 项 | 内容 |
|---|---|
| 依赖 | M9 |
| 交付 | DOC-16、cordis-up、cordis-supervisor；CP-1–CP-10；协议和输入解析模糊测试目标 |
| 测试 | 真实进程插件覆盖握手与每种消息、截止时间、取消、背压、协议错误、超大帧、畸形 CBOR、消息洪泛和 SIGKILL；验证依赖服务回收与有界重启；每个指定模糊测试目标连续运行 30 分钟 |
| 退出 | Y-1–Y-10 |

### M11 插件宿主与热替换

| 项 | 内容 |
|---|---|
| 依赖 | M10 |
| 交付 | cordis-host：库、租约、ABI 对象与 UP/1 对照表，按核心指令执行原生插件；ABI v1.0 冻结；版本与故障 fixtures |
| 测试 | 真实跨库调用、状态迁移、租约归零与关闭策略、8 线程调用；Rust 1.85 编译的插件装载；段错误、栈溢出、abort 的核心隔离；核心安排同组 A 先清理、B 后卸载且 A 随后直连新 B；宿主依赖树与内核 crate 分离 |
| 退出 | `Y-1`–`Y-10` |

### M12 核心可执行文件

| 项 | 内容 |
|---|---|
| 依赖 | M11 |
| 交付 | cordis-runtime、console-log、kv-file、示例部署；group/checkpoint_ms/requires_core 与 native 清单；检查点、服务安装；命令转交与 cordis-dev 的 Rust 脚手架；manage@1、grants、instances.toml 叠加层及 watch 插件 |
| 测试 | watch 触发配置重读、替换与回滚；有序退出与状态继承；错误配置保持运行；状态与检查点；逐方法管理授权和叠加层重启恢复；脚手架经命令转交后运行；核心依赖树与 ABI/SDK、文件监视库及开发工具分离 |
| 退出 | `Y-1`–`Y-10` |

### M13 验证门

| 项 | 内容 |
|---|---|
| 依赖 | `M12` |
| 交付 | cordis-contract、kv-contract；cordis-dev verify，通过独立 cordis run 部署执行，cordis verify 原样转交 |
| 测试 | `kv-mem` 与 `kv-file` 都通过 `kv-contract`，各自获得验证副本与印记；违反契约的 fixture 报告失败；`require_verified = true` 时缺少印记或与验证副本的完整字节比较失败的候选在 `SW-2` 被拒绝；被测库在套件运行中崩溃时，`cordis verify` 报告失败并正常退出 |
| 退出 | `Y-1`–`Y-10` |

### M14 验收

| 项 | 内容 |
|---|---|
| 依赖 | `M13` |
| 交付 | AC-1–AC-9 与 DOC-15 AU-1、AU-2、AU-8–AU-11、AU-16；RM-1–RM-6、RM-10 实测；README 构建、运行与首个插件；PROGRESS 第二阶段记录 |
| 退出 | Y-1–Y-10；本行所列场景全部通过 |

### M15 插件编写 skill

M7–M14 的实现规则由本里程碑整理为面向插件作者的操作手册，供 Claude Code 与 Codex 加载。

| 项 | 内容 |
|---|---|
| 依赖 | M14；以实际可运行的 SDK 与核心接口为准 |
| 执行者 | Codex；设计方复核 |
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
| `AC-7` | 服务和 apply 内 panic；插件段错误、栈溢出、abort | panic 在 ABI 边界报告 code 5；进程级故障由核心回收对应分组，依赖方进入 Pending，核心与其他分组继续运行 |
| `AC-8` | 用 Rust 1.85.0（本机已安装，早于 `rust-toolchain.toml` 固定的 1.96.0）构建 `counter`，装载运行 | 与 1.96.0 编译的结果行为一致；`build_info` 显示两种编译器版本 |
| `AC-9` | 在 8 线程持续调用下，对同一插件连续替换 50 次 | 无死锁、无崩溃；记录 `RM-3`、`RM-4` 的中位数与最大值、`RM-6` |

## 5 完成定义

第二阶段在以下条件全部成立时完成：

1. M7–M15 的验收分别满足退出条件，PROGRESS 保存对应记录。
2. `AC-1`–`AC-9` 全部通过。
3. RM-1–RM-6、RM-10 有实测值，DOC-15 的 AU-1、AU-2、AU-8–AU-11、AU-16 全部通过。
4. `DOC-12` 状态为“已冻结 v1.0”，`docs/abi/v1-layout.txt` 与实现一致。
5. 按根 `README.md` 的“编写第一个插件”一节，从空目录新建一个插件 crate，编译后放入运行中的 `cordis` 的插件目录，插件被装载并运行。
