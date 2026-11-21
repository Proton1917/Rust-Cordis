# Rust-cordis

Rust-cordis 是一个组件运行时，支持在应用运行期间更换插件。每个插件声明自己的依赖，激活时登记服务、监听器和清理动作。例如，笔记组件等存储服务就绪后开始读写。存储实现更换后，笔记组件重新绑定服务。这一模型源于论文《一种面向时空可组合性的编程范式》。[组件模型](docs/00-strategy.md) · [上游 Cordis](https://github.com/cordiverse/cordis)

Rust-cordis runs independently replaceable components. The kernel manages service dependencies and cleanup. Native plugins run in a host process, Python and TypeScript plugins connect over UP/1, and browser components connect through the HTTP plugin.

## 快速开始

macOS Apple Silicon 用户可从 [v0.2.6 发布页](https://github.com/Proton1917/Rust-Cordis/releases/tag/v0.2.6)取得运行时。将压缩包解开，在解包目录执行以下命令：

```sh
export PATH="$PWD/bin:$PATH"
cordis version
cordis run
```

包内示例配置会启动 HTTP 插件。在浏览器打开 [本机示例页面](http://127.0.0.1:8080/)，按 Ctrl-C 有序退出。要开发自己的插件，安装 Rust 工具链后在新的工作目录执行：

```sh
cordis new demo
cd demo
cordis new-plugin hello --rust
cargo build --target-dir target
cordis run
```

另开终端，在 `demo` 目录执行 `cordis status` 可查看 `hello` 实例的状态。源码构建、各语言入口、发布打包和停机查询见[使用指南](docs/guide.md)。

## 插件的执行位置与版本

`cordis` 是核心进程，负责实例、依赖和生命周期。一个 `cordis-host` 可以同时装载多个 Rust 插件库。**v0.2.6 默认按原生插件分别成组**，将多个实例配置为相同的 `group` 即可共用宿主。日常替换按实际依赖重建；如果宿主进程退出，同组成员一起恢复。Python 与 TypeScript 使用各自的解释器，浏览器页面经 HTTP 插件连接核心。插件之间使用版本化的服务契约。[部署与分组](docs/guide.md#让两个原生插件共享宿主) · [UP/1 协议](docs/16-up1-protocol.md)

| 版本 | 插件作者需要知道的区别 |
|---|---|
| v0.2.5 发行版 | 宿主 ABI 1.1、Rust SDK 0.2.1。部署者可在核心停机后查询各实例最后确认的程序版本。 |
| v0.2.6 发行版 | 核心 0.1.5 核对在线快照的进程归属。开发工具 0.2.5 分开管理源码和安装副本。ABI 1.2 增加原生方法布局识别，Rust SDK 0.2.2 提供库级 Tokio 上下文。 |
| 下一版设计 | 新部署默认共用原生宿主，业务方法按稳定身份分派，官方工具负责从独立源码工作区装配应用。实施状态见[版本页](docs/PROGRESS.md)。 |

## 开发入口

| 任务 | 入口 |
|---|---|
| 在 Rust 程序中直接使用组件内核 | [内核与运行示例](crates/cordis/README.md) |
| 开发进程插件 | [Python SDK](sdks/python/README.md) · [TypeScript SDK](sdks/typescript/cordis-sdk/README.md) |
| 编写页面或全栈插件 | [浏览器宿主](sdks/typescript/cordis-web/README.md) · [可运行的网页示例](examples/web/README.md) |
| 理解原生调用、热替换和资源回收 | [原生接口](docs/12-abi-v1.md) · [实例生命周期](docs/11-plugin-model.md) |
| 阅读架构设计和历史数据 | [架构决定](docs/adr/0057-shared-host-and-plugin-contract.md) · [分阶段的原始测量](docs/history/progress-2026-11-16.md) |

## 许可证

本项目采用 MIT 许可证。设计与用例参照上游 Cordis，上游版权声明随仓库保留。[LICENSE](LICENSE)
