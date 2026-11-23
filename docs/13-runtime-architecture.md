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

`cordis pack --project deployment --target aarch64-apple-darwin` 读取 `deployment/cordis.toml`。格式 2 的 `build.workspace` 可指向其他目录的 Cargo 清单。构建消息给出该工作区实际生成的可执行文件和动态库，部署目录中的清单给出各实例及其来源。

### 新清单的构建输入

下例在同一应用中装配原生存储插件与网页笔记插件。Cargo 工作区和网页工程均位于部署目录之外。

```toml
format_version = 2

[build]
workspace = "../plugins/Cargo.toml"

[build.plugins.storage]
package = "app-storage"
target = "storage_native"

[[build.web]]
name = "notes"
directory = "../web/notes"
command = ["npm", "run", "build"]
output = "dist"

[instance.store]
plugin = "storage"

[instance.notes]
plugin = "notes"
```

`build.plugins` 的键对应实例的 `plugin` 引用，`package` 是工作区包名，`target` 是 Cargo 的库或可执行目标名。省略选择器时，工具按清单中的文件名选择唯一产物。同名目标由各自独立的 Cargo 编译目录承载，打包遇到歧义会报告包名。采用预建插件的格式 2 部署可省略 `build.workspace`。

网页构建项的 `name` 同样对应 `plugin` 引用。`directory` 以部署目录为基准，工具在该目录直接运行 `command` 中的程序及参数，把 `output` 目录内容装入插件清单指定的 `web.assets`。发布配置移除源码构建输入。每份经 Cargo 构建的发布插件在同一目录保存 `cordis-build.json`，其中记录包名、包版本、Cargo 目标、目标平台、release 配置及编译器报告的包特性。

`[pack.instance_config.<实例名>]` 为发布包指定实例的完整 `config` 表。开发工具在装配前应用此配置，把实际实例参数写入包内的 `cordis.toml`。生成项目用 `[pack.instance_config.watch]` 将发布环境的 `paths` 设为空数组，watch 继续监视部署配置文件。

## 3 单次装配

开发工具读取真实 Cargo 产物记录，以包身份和目标平台定位动态库。预建插件由其公开清单定位。工具把源码声明与实际产物对应，核对宿主 ABI 及依赖的业务接口，装配运行时、插件、网页和许可证。平台库后缀、发布路径映射、签名顺序与包内相对路径由工具掌握。构建失败定位到来源包和处理阶段，已交付版本继续可用。

## 4 程序来源与恢复

运行时在部署成功时保存逐实例程序来源、安装引用和安装目录。部署工具通过公开状态接口读取最后确认的程序。业务数据库、宿主工作副本与应用发布包分别由数据所有者、宿主和部署者管理。[离线绑定接口](adr/0054-offline-source-query.md)

部署控制器保存尚待激活确认的程序引用。内核报告实例进入 Active 后，控制器从监管器取得当前纤程并确认来源。批次切换期间的激活通知在事务结束后处理。状态文件的发布任务独立读取运行情况。停机时，控制器在撤回实例前保存最后一次确认结果。

批次预检根据旧部署、目标部署和候选组件确定直接变更的实例。冻结前，监管器沿实际依赖绑定和候选服务声明扩展这个集合。单库、整组和批次共用确认流程，一个窗口同时观察受影响的纤程和候选连接。监管器分别报告候选激活结果和旧资源退役结果；部署控制器负责网页、检查点和来源文件的最终提交与恢复。

当前配置由纤程持有。一个加载纪元从纤程取得配置后持有自己的只读引用，因此旧纪元的清理回调仍能读取原配置。实例也能与监管器内存中的检查点共享同一份继承状态，跨进程交付时按 UP/1 编码。

## 5 现行入口

旧格式在传入项目根查找 `Cargo.toml`，也接受已有的装配目录。格式 2 按上述清单定位构建输入。两种格式共用描述符探测、契约验证、代码签名和归档流程。[构建实现](../crates/cordis-dev/src/pack/artifacts.rs) · [使用指南](guide.md)

## 6 错误与资源

诊断区分编译、装载、绑定、执行、提交和应答传输。独立实例各自保存成功的检查点。状态文件与标准错误输出用于观测；数据所有者记录业务提交。原生执行资源和持久业务状态按不同寿命回收。

原生宿主由进程主线程推进协议运行时，原生插件的轮询、回调和析构由独立多线程执行器负责。`--native-workers` 继续控制原生执行线程数。协议侧使用单线程调度，因此并发大帧吞吐和尾延迟作为调度变更的性能核对项。

跨进程调用的参数和返回值转移其业务缓冲所有权，资源编号在原有值树内改写；接收载荷继续持有资源租约，直到完成导入。发送队列保存包含长度前缀的完整编码帧，编码完成后释放原始值树。成功响应、远端超时和本地计时器通过同一待处理请求表确定唯一完成方，并在该临界区内更新连续超时次数，随后在锁外执行通知和连接终止。

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
