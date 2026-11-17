# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 角色

2026-11-16 起由 Claude 直接设计、实现和验证（用户决定，取代此前“Claude 只做编排、Codex 实现”的分工）。交付规则、硬性规则与常用命令以 `AGENTS.md` 为准。git commit 仍在用户要求时才做，提交消息末尾加署名行。

## 设计依据

- 入口：`AGENTS.md` 的“设计依据”表；DOC-15 优先于 DOC-10–14。
- 当前状态：`docs/PROGRESS.md`。2026-11-16 以前的记录在 `docs/history/`。
- 写设计文档与 ADR 时使用 `engineering-delivery-writing` skill。

## 第一性原理

判据见 `AGENTS.md`（用户 2026-11-13 要求）。写 ADR 时加一节“职责放置”，逐项说明新增能力为什么只能放在所选位置；ADR-0046 §0、§3.9 与 ADR-0055、ADR-0056 可作范例。方案写完后逐条用判据检查一遍：初稿常见的偏差是图方便把协议细节或机密放进核心（ADR-0046 初稿把 Cookie 放进了核心）。

身份类字段先检查会不会与其他实体同名（ADR-0046 初版让页面调用方使用前端实例名作身份，与后端实例名冲突）。

## 架构要点

- 核心进程 `cordis` 不运行插件代码（`Q0`），不链接 `cordis-abi`、`cordis-sdk`，核心 crate 无 `unsafe`。只有一个内核（`crates/cordis`）。
- 高速插口：Rust 原生动态库由 `cordis-host` 按 `cordis.toml` 的 `group` 装载，同组插件经 ABI v1 直接调用。通用插口：任何语言的进程插件；前端插件经 `http` 插件转发。
- 核心与插件进程之间只走 `UP/1`（标准输入输出上的 CBOR 帧），跨分组调用受 UP/1 容量预算约束。
- 运行中的装载、替换、回滚由持有授权的插件经 `cordis.manage@1` 请求；替换与回滚都走批次流程。

## 发布物核验

对外的东西（GitHub Release 资产、证据存档、下游应用的发布包）每次都要核验。

| 检查 | 方法 | 来历 |
|---|---|---|
| 本机路径与用户名 | 对每个文件按完整字节检索：`grep -c -a -F '/Users/' <文件>`、`grep -c -a -F '<用户名>' <文件>`，结果须为 0；`strings` 只作补充 | macOS 的 `strings` 不扫描 Mach-O 装载命令，漏掉了 v0.1.0、v0.2.0 dylib 的 `LC_ID_DYLIB` 绝对路径 |
| 私有项目内容 | 检索私有项目名（例如 MathBook）与 `target/claude-edits` | 证据存档首版混入了私有项目的迁移方案与进程列表 |
| 存档范围 | 公开存档只用明确的文件清单 | 同上 |
| 下游依赖地址 | 下游应用依赖 Rust-cordis 时写公开仓库 URL 加固定修订，不用 `file://` 路径 | 下游应用的 `file://` 依赖曾漏检 |
| 实际可用 | 从新目录下载解包，检查签名与版本号，实际启动并请求一次，确认有序退出 | v0.1.0 起的发布核验流程 |

公开仓库里的文档（ADR、DOC、skill）不写私有项目名与私有细节，只描述通用现象。

## 环境约束

- 临时文件与脚本只放在本仓库的 `target/` 下，不用 `/tmp`。验证完删除自己产生的编译缓存（如 `target/review-build`）。
- Python 一律通过 `conda run -n base python <脚本文件>` 运行。`conda run` 不传递标准输入，heredoc 内联脚本不会执行；先把脚本写入 `target/`，再运行该文件。
- zsh 中不要写没有引号的 `echo ====`；分隔符写成 `echo '-----'`。
- 用户的规则禁止对本机文件计算 SHA-256 等摘要；需要识别文件身份时用序号命名与完整字节比对。
- 测试中途会重新构建 SDK 和夹具；后台跑全量测试时不要同时修改源码，否则结果不可信。
