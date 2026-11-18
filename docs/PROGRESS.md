# 运行时版本与设计状态

| 对象 | 状态 |
|---|---|
| 已发布 | v0.2.5 提供 macOS arm64 运行时。宿主 ABI 1.1，Rust SDK 0.2.1；核心 0.1.4 可在停机后查询最后确认的程序绑定。[发布数据](measurements/release-v0.2.5.json) |
| 源码基线 | `85860d8` 将单项替换与回滚接入批次流程，按调用方在内存保存最近批次结果。该提交还包含 SDK 0.2.2、ABI 1.2 布局识别和插件库级 Tokio 上下文。 |
| 目标设计 | [架构决定](adr/0057-shared-host-and-plugin-contract.md)、[共享原生宿主](15-universal-socket.md)、[固定原生调用入口](12-abi-v1.md)、[跨目录构建与交付](13-runtime-architecture.md)。这些能力的实施状态为设计阶段。 |

下一步是用[既有验收场景](14-roadmap-phase2.md)校准新的装载和调用契约，再实现工具与宿主。原版 JS 包源码兼容、Linux 实机验收及 Windows 移植各有独立条件。旧版本的开发与测量见[历史记录](history/progress-2026-11-16.md)。
