# 运行时版本与设计状态

| 对象 | 状态 |
|---|---|
| v0.2.5 发行版 | macOS arm64，宿主 ABI 1.1，Rust SDK 0.2.1。核心 0.1.4 可在停机后查询最后确认的程序绑定。[发布数据](measurements/release-v0.2.5.json) |
| 源码基线 | `85860d8` 将单项替换与回滚接入批次流程，按调用方在内存保存最近批次结果。该提交还包含 SDK 0.2.2、ABI 1.2 布局识别和插件库级 Tokio 上下文。 |
| v0.2.6 发行版 | macOS arm64，宿主 ABI 1.2，Rust SDK 0.2.2。 核心 0.1.5 让一个状态目录由一个核心写入，在线查询核对快照所属进程。开发工具 0.2.5 生成的工程分别从源码目录与 Cargo 构建目录发现插件，把已确认的插件副本保存在 `.cordis/plugins`。 |
| v0.2.7 发行版 | macOS arm64，宿主 ABI 1.2。Rust SDK 0.2.3 将异步结果布局纳入类型化调用判断。核心 0.1.6 分开管理检查点和状态快照的暂存文件，按插件目录协调并发安装，在管理队列外等待宿主重启。开发工具 0.2.6 按平台文件名寻找预编译库，Python SDK 0.1.1 在提前取消时归还请求名额和资源引用。 |
| 目标设计 | [架构决定](adr/0057-shared-host-and-plugin-contract.md)、[共享原生宿主](15-universal-socket.md)、[固定原生调用入口](12-abi-v1.md)、[跨目录构建与交付](13-runtime-architecture.md)。这些能力的实施状态为设计阶段。 |

下一步是用[既有验收场景](14-roadmap-phase2.md)校准新的装载和调用契约，再实现工具与宿主。原版 JS 包源码兼容、Linux 实机验收及 Windows 移植各有独立条件。旧版本的开发与测量见[历史记录](history/progress-2026-11-16.md)。

核心 0.1.5 先在 `runtime.plugin_dir` 发现插件，随后按 `runtime.source_dirs` 的声明顺序查找其他来源。旧脚手架工程升级核心后，在现有 `[runtime]` 下添加 `source_dirs = ["plugins"]` 即可直接发现源码目录里的清单。保留原来的 `plugin_dir` 能继续找到先前已确认的副本；有运行历史的工程清理 Cargo 目录前，应按 `cordis status --offline` 给出的目录保留这些副本。新建工程的安装目录独立于 Cargo 目录。
