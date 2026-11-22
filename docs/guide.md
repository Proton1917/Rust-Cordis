# Rust-cordis 应用开发指南

本指南适用于 v0.3.1。核心为 0.1.11，原生宿主为 0.1.5，开发工具和 Rust SDK 为 0.3.0。原生 ABI 为 1.2，进程插件与浏览器组件通过 UP/1 连接。Python SDK 为 0.1.4，TypeScript SDK 为 0.2.3，HTTP 0.1.4 内置网页 SDK 0.1.3。watch 为 0.1.3，window 为 0.1.1。完整组件版本列在发行包的 `README.md` 中。[发行包](https://github.com/Proton1917/Rust-Cordis/releases/tag/v0.3.1)

运行时把已接纳的基础配置、部署覆盖层、已确认的程序来源和受影响的检查点作为同一次提交处理。启动时先依据磁盘上的决定记录恢复部署，再挂载实例。状态目录使用支持同目录树内硬链接和原子重命名的文件系统。[部署与恢复规格](deployment-semantics.md)

## 启动第一个插件

从发行包取出同一版本组合的 `cordis`、`cordis-host`、`cordis-dev`，将三者所在目录加入 `PATH`。从本仓库构建这些程序时可执行：

```sh
cargo build --release --target-dir target -p cordis-runtime -p cordis-host -p cordis-dev
export PATH="$PWD/target/release:$PATH"
```

在准备创建示例的目录运行：

```sh
cordis new demo
cd demo
cordis new-plugin hello --rust
cargo build --target-dir target
cordis run
```

在另一个终端进入该项目运行 `cordis status`。成功时可看到 `hello` 已激活。修改插件后重新编译，watch 向管理接口提交候选；切换成功后，依赖方取得新绑定。Ctrl-C 通知核心有序退出。

使用当前工作树编译开发工具时，可在新建项目时通过 `--sdk-path` 指向该工作树的 `crates/cordis-sdk`，使工程使用与开发工具相同的 SDK 源码。

## 让两个原生插件共享宿主

`cordis.toml` 缺省使用格式 1，省略 `group` 时按插件名分组。声明 `format_version = 2` 后，省略 `group` 的原生插件默认使用 `native` 组。两种格式都可以显式设置 `group`，进程插件与网页组件沿用自身的执行方式。让两份已装配的原生库共用指定宿主时，在各自的实例声明中填写同一组名。

```toml
[instance.storage]
plugin = "kv"
group = "editor"

[instance.counter]
plugin = "counter"
group = "editor"
```

提供方和使用方通过接口声明依赖。正常替换按实际绑定处理，组进程退出时同组成员共同恢复。

## 提交管理操作与查询结果

新管理插件调用 `cordis.manage@2.0`，并在实例的 `grants` 中用 `cordis.manage@2` 授权所需方法。`install` 返回安装后的候选引用。`add`、`remove`、`replace`、`rollback`、`configure`、`reload`、`replace_batch` 和 `shutdown` 返回带 `id` 的受理票据。受理应答写出后，核心把操作交给管理队列。包含配置重读的批次使用 `replace_batch` 和 `reload` 两项授权。

管理实例调用 `result(id)` 查询本实例的票据，操作者也可以执行 `cordis status --caller <实例名> --ticket <票据 ID>` 读取发布到状态页的结果。票据记录在当前进程内有界保留。重启后用离线查询核对已确认的程序来源。

| 票据状态 | 含义 |
|---|---|
| `pending` | 已受理，等待执行结果。 |
| `succeeded` | 操作成功。 |
| `committed_with_cleanup` | 部署已经提交，旧资源退役或暂存清理出了错。 |
| `rejected` | 操作被拒绝或执行失败，见票据的 `code` 和 `error`。 |
| `restored` | 候选执行失败，运行时已恢复先前部署。 |
| `recovery_required` | 运行时需要恢复磁盘决定或原部署，期间暂停新的结构变更和检查点覆盖。 |

`cordis.manage@1.1` 保留三个兼容方法。`install` 返回安装引用，`replace_batch` 接受原请求编号并返回票据，`shutdown` 用空应答表示受理。旧接口使用 `cordis.manage@1` 授权，2.0 接口优先使用 `cordis.manage@2` 授权，缺省时沿用已有的 `cordis.manage@1` 授权。其他管理方法和详细查询使用 2.0 接口。[部署与恢复规格](deployment-semantics.md)

## 契约与语言

接口从 WIT 生成 Rust、Python、TypeScript 绑定。在本仓库根目录可为 KV 接口生成 TypeScript 声明：

```sh
cargo run -p cordis-dev -- idl gen --lang ts --output target/kv.ts plugins/kv-api/kv.wit
```

Rust 服务由 `#[interface]` 或生成接口表达。同步方法中的异步等待可交给 `Context::wait`，异步方法直接使用 `.await`。Rust SDK 的 `tokio` 特性为需要 Tokio 网络驱动等能力的库提供执行上下文。插件可在导出声明中设置 `tokio = false`，按宿主提供的上下文执行。[Rust 接口](../skills/cordis-first-principles/references/rust-api.md) · [等待与任务](adr/0051-native-synchronous-wait.md)

生成 Python 插件后安装该项目的实际依赖：

```sh
cordis new-plugin greeter --python
conda run -n base python -m pip install --target plugins/greeter/.dependencies -r plugins/greeter/requirements.txt
```

使用 TypeScript 的项目则可执行 `cordis new-plugin greeter --ts`，随后在该插件目录安装依赖并运行其 build 脚本。两个示例独立使用同一个插件名。脚本通过标准输入输出交换 UP/1 帧，文本日志经 SDK 发送。[Python SDK](../sdks/python/README.md) · [TypeScript SDK](../sdks/typescript/cordis-sdk/README.md)

## 网页与全栈

`cordis new-plugin notes --web` 生成浏览器模块；`--rust --web`、`--python --web`、`--ts --web` 生成相应的全栈组件。HTTP 插件提供静态资源和 WebSocket 入口，运行配置给 HTTP 实例设置 `forwarder = true`。页面关闭时撤回本页面登记的服务；换版本时，页面组件可以继承导出的草稿。[可运行网页示例](../examples/web/README.md) · [实际页面状态](design/flow/index.html)

## 通用字节流与调用来源

HTTP 插件解释路由、方法、请求头、正文和会话。处理方从可信调用元数据读取后端实例或页面的 `page_of`。密钥插件按实例授权读取配置的键。大文件和持续增长的结果通过 `cordis.byte-stream@1.0` 分块取得；请求完成由操作编号或快照标识表达。返回失败时先查询业务所有者的持久结果，再决定是否重提。[HTTP、资源与身份](adr/0046-http-routes.md)

## 验证与发布

以 macOS 的 KV 提供方和契约套件为例，在本仓库根目录执行：

```sh
cargo build --release --target-dir target -p kv-mem -p kv-contract
cordis verify --plugin target/release/libkv_mem.dylib --suite target/release/libkv_contract.dylib
```

为已准备好的应用生成发行包时，从应用目录执行：

```sh
cordis pack --project . --target aarch64-apple-darwin --out dist
```

格式 2 使用 `build.workspace` 定位 Cargo 清单。新生成项目默认指向自己的 `Cargo.toml`。部署文件与源码分开时，可在 `build.workspace` 填写相对于部署目录的路径，并用 `build.plugins` 指定同名产物的来源包。源目录中的预编译插件和网页构建结果也可随包交付。格式 1 沿用项目根的 Cargo 清单，`runtime.source_dirs` 可列出两种格式的附加插件目录。[格式 2 示例与产物信息](13-runtime-architecture.md) · [脚本解释器布局](adr/0043-bundled-script-runtimes.md)

将生成的归档解开到部署目录，再进入包内目录执行 `./cordis run`。用另一终端运行 `./cordis status` 查看实例状态。新生成项目的 watch 在开发目录监视构建产物和配置文件，在发布目录监视包内的 `cordis.toml`。[打包规则](adr/0036-release-packaging.md)

## 停机后的程序绑定查询

`cordis status --offline --config cordis.toml` 输出最后确认的程序绑定。退出码 0 表示读取成功，1 表示配置或记录读取失败，2 表示参数错误。先检查 `schema_version` 为 1，再从 `instances` 按名称读取 `declared_source`、`installed_reference` 与 `plugin_directory`。有效的空映射与读取失败分别处理。确认中的候选沿用上一份已提交指向；部署者根据当前引用、前一版本和其他使用方决定保留范围。[公开查询契约](adr/0054-offline-source-query.md)

### 升级与恢复

升级前先有序停机，并一起保留 `cordis.toml`、整个状态目录、已确认的插件安装目录与业务数据。状态目录含已接纳的基础配置、来源文件、检查点和可能存在的 `.deployment-undo`、`.deployment-staging` 记录。核心先恢复已提交的部署，再通过与 `reload` 相同的事务应用本次 `cordis.toml` 的变化。候选被拒绝时，已恢复的部署继续运行，`cordis status` 的 `configuration_error` 显示原因。运行中仍可提交 `reload`，或由 watch 提交包含重读的批次。

启动与状态查询根据本次 `cordis.toml` 定位 `runtime.state_dir`。配置语法错误或状态目录定位字段无效时，命令直接报告错误。首次启动或旧状态目录缺少已接纳基础配置时，部署使用本次有效配置。改用另一状态目录会选择该目录的部署记录，迁移时需事先把对应的状态和安装副本准备到目标位置。宿主超时等用于初始化监管器的运行参数随下一次启动采用已接纳的值。

实例级 `rollback(name)` 选回上一个程序引用，并交接当前导出的业务状态。回退核心发行版时，使用与旧核心配套的升级前配置、状态目录及插件副本，并按应用自己的数据恢复约定处理业务数据库。出现 `recovery_required` 时，保留当前日志和完整状态目录，按同一版本完成恢复，再用离线查询核对已确认来源。[部署与恢复规格](deployment-semantics.md)

<a id="里程碑验收摘要m0m18"></a>

## 版本与验收资料

v0.3.0 提供 macOS arm64 发行资产，本次运行与性能记录也来自该平台。开发工具支持在 Linux x86_64 目标机器上打包，Linux PSS 和时延留待该平台测量。实现量统计分别列出手写生产代码、构建工具、交付模板和生成源码，性能结果按各场景的样本与波动报告。[v0.3.0 实测与统计](measurements/deployment-revision.md) · [历史验收](history/progress-2026-11-16.md)
