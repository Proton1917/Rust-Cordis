# Rust-cordis 应用开发指南

本指南的命令适用于已发布的 v0.2.5。新架构中的共享默认分组、固定原生入口和跨目录构建契约见[运行时设计](15-universal-socket.md)。v0.2.5 使用宿主 ABI 1.1 和 Rust SDK 0.2.1；工具、核心与 SDK 各有自己的组件版本。

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

## 让两个原生插件共享宿主

对部署文件里两个已有实例分别设置相同的 `group`。下面展示实例字段，示例中的库由应用事先装配。

```toml
[instance.storage]
plugin = "kv"
group = "editor"

[instance.counter]
plugin = "counter"
group = "editor"
```

提供方和使用方仍要通过接口声明依赖。该设置只改变执行位置。正常替换按依赖图处理；组进程退出时两者共同重建。v0.2.5 对省略 `group` 的原生库分别成组。

## 契约与语言

接口从 WIT 生成 Rust、Python、TypeScript 绑定。在本仓库根目录可为 KV 接口生成 TypeScript 声明：

```sh
cargo run -p cordis-dev -- idl gen --lang ts --output target/kv.ts plugins/kv-api/kv.wit
```

Rust 服务由 `#[interface]` 或生成接口表达。同步方法中的异步等待可交给 `Context::wait`；异步方法直接 `.await`。v0.2.5 中依赖 Tokio 网络驱动的插件需要自己的执行上下文；库级自动上下文属于随后主分支的 SDK 0.2.2。[Rust 接口](../skills/cordis-first-principles/references/rust-api.md) · [等待与任务](adr/0051-native-synchronous-wait.md)

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

`pack` 使用项目根的 Cargo 配置及产物记录，也可装配已有插件清单。源码和部署文件分开的 v0.2.5 项目需先从真正的源码工作区构建，再把原生库、描述和网页资源交给部署目录。下一版把这一步并入工具的公开输入。与发布包一起装入的 Python、Node、网页资产及签名约束见[打包规则](adr/0036-release-packaging.md)与[脚本解释器布局](adr/0043-bundled-script-runtimes.md)。

## 停机后的程序绑定查询

`cordis status --offline --config cordis.toml` 输出最后确认的程序绑定。退出码 0 表示读取成功，1 表示配置或记录读取失败，2 表示参数错误。先检查 `schema_version` 为 1，再从 `instances` 按名称读取 `declared_source`、`installed_reference` 与 `plugin_directory`。有效的空映射与读取失败分别处理。确认中的候选沿用上一份已提交指向；部署者根据当前引用、前一版本和其他使用方决定保留范围。[公开查询契约](adr/0054-offline-source-query.md)

<a id="里程碑验收摘要m0m18"></a>

## 版本与验收资料

第一阶段的验收对象为进程内组件。第二阶段增加独立原生库与部署管理，后续阶段扩展到进程插件、网页、外部 HTTP 和字节流。各版本的运行条件和样本在[历史页](history/progress-2026-11-16.md)中分别列出。
