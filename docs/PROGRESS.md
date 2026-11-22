# 运行时版本与设计状态

| 对象 | 状态 |
|---|---|
| v0.2.5 发行版 | macOS arm64，宿主 ABI 1.1，Rust SDK 0.2.1。核心 0.1.4 可在停机后查询最后确认的程序绑定。[发布数据](measurements/release-v0.2.5.json) |
| 源码基线 | `85860d8` 将单项替换与回滚接入批次流程，按调用方在内存保存最近批次结果。该提交还包含 SDK 0.2.2、ABI 1.2 布局识别和插件库级 Tokio 上下文。 |
| v0.2.6 发行版 | macOS arm64，宿主 ABI 1.2，Rust SDK 0.2.2。 核心 0.1.5 让一个状态目录由一个核心写入，在线查询核对快照所属进程。开发工具 0.2.5 生成的工程分别从源码目录与 Cargo 构建目录发现插件，把已确认的插件副本保存在 `.cordis/plugins`。 |
| v0.2.7 发行版 | macOS arm64，宿主 ABI 1.2。Rust SDK 0.2.3 将异步结果布局纳入类型化调用判断。核心 0.1.6 分开管理检查点和状态快照的暂存文件，按插件目录协调并发安装，在管理队列外等待宿主重启。开发工具 0.2.6 按平台文件名寻找预编译库，Python SDK 0.1.1 在提前取消时归还请求名额和资源引用。 |
| v0.2.8 发行版 | macOS arm64，核心 0.1.7、宿主 0.1.3、Rust SDK 0.2.4。内核修正实例清理和并发依赖通知；宿主处理卸载结果、候选拒绝和库记录回收。Python SDK 0.1.2 按期返回超时，TypeScript SDK 0.2.1 和网页 SDK 0.1.1 改进分片接收。[审查结果与测量](measurements/v0.2.8-review.md) |
| v0.2.9 发行版 | 核心 0.1.8 同步提交程序来源、部署配置与检查点，并撤回挂载失败的后端。监管器 0.1.5 在配置恢复后解冻原实例，在库释放出错后查询回收阈值。TypeScript SDK 0.2.2 修复排队超时和后台任务报错，HTTP 0.1.3 内置网页 SDK 0.1.2。 |
| v0.2.10 发行版 | 核心 0.1.9 协调离线读取与部署恢复，在停机撤回实例前保存已激活的来源。 |
| v0.3.0 发行版 | macOS arm64，核心 0.1.10、宿主 0.1.4、开发工具 0.3.0、Rust SDK 0.3.0。格式 2 默认共用原生宿主，`cordis.manage@2.0` 返回可查询票据。部署决定统一处理配置、程序来源和检查点，开发工具支持跨目录应用打包。原生 ABI 沿用 1.2。[发布页](https://github.com/Proton1917/Rust-Cordis/releases/tag/v0.3.0) · [部署修订与实测](measurements/deployment-revision.md) |

支撑 v0.3.0 发行的运行验证与对照测量来自 macOS arm64。Linux x86_64 的 PSS 和时延留待该平台测量。实现量报告分别统计手写生产代码、构建工具、交付模板和生成源码。固定入口实验止于标量门槛。[测量范围与统计明细](measurements/deployment-revision.md)

既有验收场景见[运行时验收范围](14-roadmap-phase2.md)，原版 JS 包源码兼容见[兼容专项](17-upstream-ecosystem-compat.md)，旧版本的开发与测量见[历史记录](history/progress-2026-11-16.md)。

核心 0.1.5 先在 `runtime.plugin_dir` 发现插件，随后按 `runtime.source_dirs` 的声明顺序查找其他来源。旧脚手架工程升级核心后，在现有 `[runtime]` 下添加 `source_dirs = ["plugins"]` 即可直接发现源码目录里的清单。保留原来的 `plugin_dir` 能继续找到先前已确认的副本；有运行历史的工程清理 Cargo 目录前，应按 `cordis status --offline` 给出的目录保留这些副本。新建工程的安装目录独立于 Cargo 目录。
