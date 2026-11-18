# 运行时的实现与构建职责

## 1 模块与事实来源

| 所有者 | 职责 |
|---|---|
| `cordis` 内核库 | 上下文、实例、绑定、效应与生命周期 |
| `cordis-runtime`、`cordis-supervisor` | 部署、进程监管、状态交接、已确认来源与离线查询 |
| `cordis-up`、`cordis-routing`、`cordis-channels` | 帧、双向调用、对象和转发通道 |
| `cordis-host`、`cordis-abi` | 原生装载、执行、缓冲与库租约 |
| SDK、宏和 `cordis-idl` | 业务接口、语言绑定与资源包装 |
| `cordis-dev` | 工程生成、接口生成、插件构建、契约验证与应用打包 |
| HTTP 插件、网页 SDK、`cordis-web-deployment` | 网络协议、页面执行、前后端实例装配 |

## 2 构建与部署目录

目标工具接受部署清单、源码工作区、目标平台、锁定运行时和应用资源声明。相对路径从声明它的清单解析。例如应用可以有 `plugins/Cargo.toml`、`web/` 与 `deployment/cordis.toml`。Cargo 工作区负责报告编译产物，网页工程交付静态资源，部署描述给出实例及其来源。应用只维护拥有这些事实的文件。

### 新清单的构建输入

下例是目标格式的目录约定。部署清单放在 `deployment/cordis.toml`，业务 Cargo 工作区与网页目录是它的同仓库兄弟目录。独立构建步骤使用明确的参数数组，工作目录按各自字段定位。

```toml
format_version = 2

[build]
workspace = "../plugins/Cargo.toml"

[[build.web]]
name = "notes"
directory = "../web/notes"
command = ["npm", "run", "build"]
output = "dist"
```

`workspace` 相对部署清单解析。`directory` 指向网页工程，`output` 相对该工程解析。Rust 插件通过 Cargo 包名和目标身份选定产物；目标平台与配置来自本次构建。部署引用、源包身份和实际产物一一对应。两个候选产物都满足同一声明时，工具报告歧义并列出候选，应用通过显式源身份消除歧义。预建插件直接声明已交付的清单与版本，不触发源码构建。

## 3 单次装配

开发工具读取真实 Cargo 产物记录，以包身份和目标平台定位动态库。预建插件由其公开清单定位。工具把源码声明与实际产物对应，核对宿主 ABI 及依赖的业务接口，装配运行时、插件、网页和许可证。平台库后缀、发布路径映射、签名顺序与包内相对路径由工具掌握。构建失败定位到来源包和处理阶段，已交付版本继续可用。

## 4 程序来源与恢复

运行时在部署成功时保存逐实例程序来源、安装引用和安装目录。部署工具通过公开状态接口读取最后确认的程序。业务数据库、宿主工作副本与应用发布包分别由数据所有者、宿主和部署者管理。[离线绑定接口](adr/0054-offline-source-query.md)

## 5 现行入口

发布版 `pack` 在传入项目根查找 `Cargo.toml`，也接受已有的装配目录。应用将源码与部署目录分开时，当前流程需显式交接编译产物。目标设计将源码工作区位置作为公开输入，由工具完成交接。[现行源码](../crates/cordis-dev/src/pack/artifacts.rs) · [使用指南](guide.md)

## 6 错误与资源

诊断区分编译、装载、绑定、执行、提交和应答传输。独立实例各自保存成功的检查点。状态文件与标准错误输出用于观测；数据所有者记录业务提交。原生执行资源和持久业务状态按不同寿命回收。

## 7 依赖白名单

运行时依赖及其使用位置如下。新增运行时依赖由对应模块说明用途并记录架构决定。

| crate | 用途 | 使用者 |
|---|---|---|
| `serde` | 配置、DTO、状态块和构建映射规则的序列化 | `cordis-abi`、`cordis-sdk`、`cordis-runtime`、`cordis-types`、`cordis-up`、`cordis-routing`、`cordis-web-deployment`、`cordis-dev`、插件 |
| `shlex` | 保留 C/C++ 构建参数的分词与引号语义 | `cordis-dev`、`cordis-testkit` |
| `serde_json` | JSON 验证印记、状态、报告与页面启动信息 | `cordis-host`、`cordis-runtime`、`cordis-dev`、`plugins/http` |
| `semver` | 插件版本解析与接口数值版本处理 | `cordis-host`、`cordis-supervisor`、`cordis-routing`、`cordis-runtime`、`cordis-dev` |
| `clap` | 命令行参数与子命令解析 | `cordis-runtime`、`cordis-host`、`cordis-dev` |
| `ciborium` | CBOR 编解码（`ADR-0010`） | `cordis-abi`、`cordis-sdk`、`cordis-host`、`cordis-up` |
| `libloading` | 打开动态库 | `cordis-host` |
| `tokio` | 执行器、计时、双向消息、子进程 I/O、路由退役通知（ADR-0052）、插件自有网络运行时与插件库级运行时上下文（ADR-0055，`cordis-sdk` 的可选 feature） | `cordis-sdk`、`cordis-runtime`、`cordis-supervisor`、`cordis-routing`、`cordis-channels`、`cordis-web-deployment`、`cordis-host`、`cordis-up`、`cordis-process`、`cordis-dev`、`plugins/http`、`plugins/window` |
| `nix` | 安全的 POSIX 进程组信号接口（ADR-0021） | `cordis-process` |
| `rustix` | 安全的 waitid 状态观察与待回收子进程管理（ADR-0027） | `cordis-process` |
| `libfuzzer-sys` | CP-9 输入模糊测试（ADR-0021） | 独立 fuzz 工作区的开发构建 |
| `notify` | 目录监视 | `plugins/watch` |
| `uuid` | 随机批次请求标识与页面会话令牌（ADR-0031、ADR-0038） | `plugins/watch`、`plugins/http` |
| `chrono` | 终端日志时间格式（ADR-0032） | `plugins/console-log` |
| `toml` | 解析 `cordis.toml` 与生成项目配置 | `cordis-runtime`、`cordis-dev` |
| `syn`、`quote`、`proc-macro2` | 过程宏 | `cordis-sdk-macros` |
| `wit-parser`、`heck`、`pulldown-cmark`、`syn`、`prettyplease` | WIT 解析、命名、文档属性与 Rust 源码生成（ADR-0039） | `cordis-idl`，由 `cordis-dev` 调用 |
| `scoped-tls` | 同步 Serde 调用期资源环境（ADR-0039） | `cordis-abi` |
| `toml_edit` 0.25.19 | 保留项目注释与格式的脚手架配置修改（ADR-0040） | `cordis-dev` |
| `parking_lot`、`futures-util`、`thiserror` | 与内核相同 | 各 crate |
| `object` 0.41.0 | 读取发布产物的 ELF/Mach-O 机器类型（ADR-0036） | `cordis-dev` |
| `tar` 0.4.46、`flate2` 1.1.11 | 生成 tar.gz 发布包（ADR-0036） | `cordis-dev` |
| `plist` 1.10.3、`pathdiff` 0.2.3 | Bundle/entitlement 文件与包内相对路径（ADR-0036） | `cordis-dev` |
| `tao` 0.37.2、`wry` 0.57.2、`muda` 0.21.2 | 窗口事件循环、系统 WebView 与原生菜单（ADR-0037） | `plugins/window` |
| `axum` 0.8.9、`tower` 0.5.3、`tower-http` 0.7.1 | HTTP/WebSocket 路由、静态文件和响应（ADR-0038、ADR-0041） | `plugins/http` |
| `url` 2.5.8 | 页面地址与版本资源 URL（ADR-0038、ADR-0041） | `plugins/http`、`cordis-web-deployment` |
| `dotenv-parser` 0.1.3 | 密钥文件的 .env 语法与字面值解析（ADR-0046 §3.10） | `plugins/secrets` |

开发依赖按相应测试或工具构建声明。
