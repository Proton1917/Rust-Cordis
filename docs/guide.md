# Rust-Cordis 使用指南

本文是 [README](../README.md) 的配套手册，收录内核接口、构建检查、插件编写、契约验证、服务部署、发布打包、WIT 生成，以及 Python、TypeScript 和网页插件的操作说明。项目定位、架构和实测数据见 README。

## 目录

1. [内核概念与接口](#内核概念与接口)
2. [内核示例](#内核示例)
3. [内核设计约束](#内核设计约束)
4. [构建与检查](#构建与检查)
5. [运行核心与编写第一个插件](#运行核心与编写第一个插件)
6. [契约验证](#契约验证)
7. [验证插件与部署为服务](#验证插件与部署为服务)
8. [发布压缩包与 macOS 应用](#发布压缩包与-macos-应用)
9. [WIT 接口生成](#wit-接口生成)
10. [Python 进程插件](#python-进程插件)
11. [TypeScript 进程插件](#typescript-进程插件)
12. [网页插件与全栈插件](#网页插件与全栈插件)
13. [里程碑验收摘要（M0–M18）](#里程碑验收摘要m0m18)

## 内核概念与接口

| 论文概念 | 上游（TypeScript） | 本仓库（Rust） |
|---|---|---|
| 可回退效应 | `ctx.effect(() => dispose)` | `ctx.effect(label, \|fx\| { fx.defer(..) })`、`ctx.effect_async` |
| 组件与纤程 | `ctx.plugin(fn, config)` | `ctx.mount_with(&def, config)`，返回 `Fiber` |
| 配置更新、重启、释放 | `fiber.update` / `restart` / `dispose` | 同名方法，返回可等待的任务 |
| 事件 | 字符串事件名，`emit` / `parallel` / `serial` / `bail` / `waterfall` | 以类型为键的事件，分同步、异步、瀑布三族 |
| 反应式余效应（服务） | `ctx.provide` / `ctx.foo` / `inject` | 类型化服务键 `ServiceKey`，`ctx.provide` / `ctx.get` / `Inject` |
| 隔离域 | `ctx.isolate(name)` | `ctx.isolate::<K>(realm)` |
| 拦截元数据 | `ctx.intercept(name, config)` | `ctx.intercept::<K>(meta)` |

完整的映射表在 `docs/02-architecture.md` §7。

## 内核示例

下面的程序展示可回退效应和组件的加载、更新、释放。完整源码位于 [`crates/cordis/examples/readme.rs`](../crates/cordis/examples/readme.rs)，在工作区根目录执行 `cargo run -p cordis --example readme`。

```rust
use cordis::{ComponentDef, Context, Inject};
use cordis_testkit::TestExecutor;

fn main() -> cordis::Result {
    // 宿主提供任务派发与时钟；这里用 testkit 的确定性执行器。
    let host = TestExecutor::new();
    let root = Context::new(host.clone());

    // 可回退效应：效应体登记逆变换，释放时按相反顺序执行。
    let effect = root.effect("greeting", |fx| {
        println!("effect on");
        fx.defer(|| {
            println!("effect off");
            Ok(())
        })
    })?;
    host.finish(effect.dispose())?;

    // 组件：激活时贡献效应；重载或停用时由内核完整回退。
    let server = ComponentDef::new("server", Inject::new(), |ctx, port: u16| async move {
        println!("listen {port}");
        ctx.effect("listener", move |fx| {
            fx.defer(move || {
                println!("close {port}");
                Ok(())
            })
        })?;
        Ok(())
    });
    let fiber = root.mount_with(&server, 8080u16)?;
    host.finish(fiber.settled())?;
    host.finish(fiber.update(9090u16))?;
    host.finish(fiber.dispose())?;
    host.finish(root.fiber().dispose())?;
    Ok(())
}
```

输出：

```
effect on
effect off
listen 8080
close 8080
listen 9090
close 9090
```

## 内核设计约束

| 约束 | 内容 |
|---|---|
| 宿主无关 | 内核不依赖 tokio。任务派发与时间由宿主通过 `Spawn`、`Clock` 两个 trait 注入 |
| 线程安全 | 公开句柄均为 `Send + Sync`。内核状态由一把锁保护；持锁期间不调用用户代码、不轮询用户 `Future` |
| 协作式取代 | 过时的加载运行到检查点或结束，再由内核回退。内核不通过丢弃 `Future` 取消加载 |
| 显式清理 | 清理由效应的逆变换完成，不依赖 `Drop`。根上下文需要显式 `dispose` |
| 无 `unsafe` | 内核 crate 不使用 `unsafe` |
| 规模 | 内核目标 4800 代码行，上限 6000；空行与注释分别统计，见 DOC-13 §1.1 |
| 依赖 | 运行时依赖取自白名单：`slotmap`、`parking_lot`、`futures-util`、`thiserror` |

各项约束的依据见 `docs/00-strategy.md` 与 `docs/adr/`。

## 构建与检查

工具链版本由 `rust-toolchain.toml` 固定为 Rust 1.96.0。

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
conda run -n base python tools/conformance.py check   # 校验一致性矩阵
```

原生验收会自动构建真实插件库，并使用 Rust 1.85.0 验证较旧编译器生成的插件。开发与运行工具可以一起构建：

```bash
cargo build --workspace --target-dir target
target/debug/cordis version
target/debug/cordis new target/hello-cordis --sdk-path "$PWD/crates/cordis-sdk"
cd target/hello-cordis
cargo build --target-dir target
../../target/debug/cordis run
```

生成的项目包含 app 与 watch。watch 合并一个防抖窗口内的构建产物和配置变化，核心统一检查并替换，失败恢复原配置、原版本及原状态。管理授权写在 `cordis.toml`，运行中变更保存在状态目录的叠加层；批次受理票据与实际结果分别记录，watch 自身重建后继续处理待确认窗口。

文件 KV、计数组件和终端日志的部署示例位于 [`examples/native`](../examples/native/cordis.toml)：

```bash
bash examples/native/build.sh
cd examples/native
./cordis run
```

另一个终端可在该目录执行 `./cordis status` 读取实例与库状态。`SIGTERM` 或 `SIGINT` 触发最终状态导出和有序退出；再次运行时继承保存的状态。

## 运行核心与编写第一个插件

在仓库根目录构建运行程序，并把它们所在目录加入当前终端的 PATH：

```bash
cargo build --release --target-dir target -p cordis-runtime -p cordis-host -p cordis-dev
export PATH="$PWD/target/release:$PATH"
```

`cordis` 负责运行和状态管理，`cordis-host` 在独立进程中装载原生插件，`cordis-dev` 执行开发命令。三者放在同一目录时，核心可以直接找到宿主和开发工具。

从新目录创建插件项目：

```bash
cordis new demo
cd demo
cordis new-plugin hello --rust
cargo build --target-dir target
cordis run
```

项目包含 `app`、`watch` 和新建的 `hello`。SDK 依赖固定到开发工具构建时的 Git 修订；在本仓库开发 SDK 时，可以在仓库根目录执行 `cordis new demo --sdk-path "$PWD/crates/cordis-sdk"`。构建命令中的 `--target-dir target` 使动态库落在项目配置使用的 `target/debug`，也适用于设置了全局 `CARGO_TARGET_DIR` 的环境。

另开一个终端，进入同一个 `demo` 目录执行 `cordis status`，可以在实例列表中查看 `hello` 的 `active` 状态。修改 `plugins/hello/src/lib.rs` 后再次执行 `cargo build --target-dir target`，`watch` 会把同一防抖窗口的变化作为一批提交给管理接口。批次失败时，运行配置、插件版本和原状态共同恢复。Ctrl-C 触发有序退出并保存检查点。

生成的 `hello` 组件在激活时发送日志事件；部署 `console-log` 插件可以把这些事件显示在终端。服务、效应、隔离域、状态导出和管理授权的接口见 [插件模型](11-plugin-model.md) 与 [SDK 接口](13-runtime-architecture.md#4-sdk-接口)。

[Cordis 插件编写手册](../skills/cordis-plugin/SKILL.md) 说明共享接口、服务、事件、效应清理、冻结状态、契约验证与替换，并提供可复制的接口、提供方和契约套件模板。Claude Code 与 Codex 可加载同一份 skill；`cordis new-plugin` 直接读取其中的组件模板。

## 契约验证

`cordis verify` 转交独立开发工具，启动隔离的核心和宿主执行真实套件。下面的命令验证内存 KV 的读写、键顺序、flush 和状态继承：

```bash
mkdir -p target/verify-demo
target/debug/cordis verify \
  --plugin target/debug/libkv_mem.dylib \
  --suite target/debug/libkv_contract.dylib \
  --project target/verify-demo
```

Linux 使用相应的 `.so` 文件名。验证成功后，命令保存已验证副本和库旁的 `.verified.json` 印记，逐项输出断言结果与验证时长。文件 KV 可通过 `--config` 指定包含 `path` 的 TOML 配置文件。

部署设置 `runtime.require_verified = true` 后，宿主在打开候选库前比较完整字节，握手后核对实际提供接口的契约覆盖。安装引用和宿主缓存库携带印记，有状态重启继续执行同一检查。

## 验证插件与部署为服务

契约套件在独立核心中运行。下面的命令在仓库根目录构建并验证内存 KV 实现：

```bash
cargo build --release --target-dir target -p kv-mem -p kv-contract
cordis verify --plugin target/release/libkv_mem.dylib --suite target/release/libkv_contract.dylib
```

macOS 动态库扩展名为 `.dylib`，Linux 使用 `.so`。验证成功后，工具保存 `.cordis/verified/` 中的完整库副本，并在被测库旁写入 `.verified.json`。配置 `require_verified = true` 后，宿主在打开候选库前比较完整字节，再核对接口契约覆盖范围。

在部署项目目录中执行 `cordis service install --name com.example.cordis.demo`，macOS 会把配置写入 `~/Library/LaunchAgents/`（root 使用 `/Library/LaunchDaemons/`）并启动服务；Linux 使用 systemd 用户单元目录。服务在后续登录时继续启动。使用 `--output service/demo.plist` 可在指定位置生成配置供检查；`--dry-run` 默认生成在项目的 `service/` 目录。核心意外退出后由服务管理器重新启动，插件按检查点恢复；状态文件和插件数据保存在项目的 `state/` 中。

执行 `cordis service uninstall --name com.example.cordis.demo` 可停止服务、删除安装配置，并查看实际删除的路径。

## 发布压缩包与 macOS 应用

`cordis pack` 在目标平台执行构建和装配，目前接受 `aarch64-apple-darwin` 与 `x86_64-unknown-linux-gnu`。部署配置中的 `instance.plugin` 使用具体动态库路径或带 `cordis-plugin.toml` 的插件目录。下面在仓库根目录准备一个实际内存 KV 部署：

```bash
cargo build --release --target-dir target \
  -p cordis-runtime -p cordis-host -p cordis-dev -p kv-mem
mkdir -p target/pack-demo/plugins
cp target/release/libkv_mem.dylib target/pack-demo/plugins/
```

保存以下内容为 `target/pack-demo/cordis.toml`：

```toml
requires_core = "^0.1"

[pack]
name = "cordis-demo"
version = "0.1.0"

[runtime]
plugin_dir = "plugins"

[instance.store]
plugin = "libkv_mem.dylib"
component = "kv-store"
```

执行发布命令：

```bash
target/release/cordis pack \
  --project target/pack-demo \
  --target aarch64-apple-darwin \
  --runtime-dir "$PWD/target/release" \
  --out dist/
```

产物为 `target/pack-demo/dist/cordis-demo-0.1.0-aarch64-apple-darwin.tar.gz`。解开后包含核心、宿主、插件、部署配置和 `service/install.sh`；在解开的目录执行 `./cordis run` 即可启动。Linux 使用 `.so` 库与 `x86_64-unknown-linux-gnu` 目标。有 Cargo 清单的插件项目会按指定 target 构建 release，工具从 Cargo 结构化产物记录取得库和可执行文件。

macOS 桌面部署可继续加入 HTTP 和窗口插件：

```bash
cargo build --release --target-dir target -p cordis-http -p cordis-window
mkdir -p target/pack-demo/plugins/window
cp target/release/libcordis_http.dylib target/pack-demo/plugins/
cp target/release/cordis-window plugins/window/cordis-plugin.toml \
  target/pack-demo/plugins/window/
```

在同一份部署配置中追加：

```toml
[instance.http]
plugin = "libcordis_http.dylib"
component = "http"

[instance.window]
plugin = "window"
component = "window"
grants = { "cordis.manage@1" = ["shutdown"] }
config = { title = "Cordis Demo" }
```

```bash
target/release/cordis pack \
  --project target/pack-demo \
  --target aarch64-apple-darwin \
  --runtime-dir "$PWD/target/release" \
  --app --out applications/
```

生成的 `target/pack-demo/applications/cordis-demo.app` 从自身位置读取部署配置，窗口通过 `cordis.web@1.0` 取得 HTTP 启动页。HTTP 的 `assets` 可指定静态目录；同时把该相对路径加入 `pack.assets`，页面资源便随发布包交付。状态默认保存到用户 `Library/Application Support/org.cordis.cordis-demo`，也可以通过 `pack.state_dir` 指定包外绝对目录；安装的新插件和宿主缓存分别位于该状态目录的 `plugins/` 与 `.cordis/lib/`。

`--sign` 指定本机签名身份并启用 hardened runtime，`--allow-third-party` 为宿主设置第三方库装载权限。启用 `require_verified` 的部署在签名改变库字节后重新执行原契约套件，验证副本及真实结果一起发布。配置、签名及可移动验证副本的规则见 [ADR-0036](adr/0036-release-packaging.md)。

`pack --target-dir <目录>` 指定复用的 Cargo 编译目录，缺省为部署项目的 `target/`；相对目录以部署项目为基准。

### 附带独立 Python 与 Node

将与目标系统及 CPU 匹配的完整 Python、Node 发行目录分别放入项目的 vendor/python、vendor/node。运行时根目录提供 bin/python3、bin/node，连同标准库、原生扩展、动态库和许可证一起参与装配。插件的 Python 依赖、TypeScript 构建结果和生产 npm 依赖在打包前准备。

```toml
[pack]
name = "script-app"
version = "0.1.0"
runtimes = ["python", "node"]

[pack.runtime_paths]
python = "vendor/python"
node = "vendor/node"
```

```bash
cordis pack --project . --target aarch64-apple-darwin --out dist --sign -
```

打包器保持发行目录内链接的归属，改写插件清单中的解释器命令，使程序从包内 runtimes 启动。签名按实际解释器入口分配权限，先处理深层 Mach-O，再处理程序和外层 Bundle。Node 使用 JIT 权限；ad-hoc 身份还为 Python/Node 设置装载包内原生扩展所需的权限。规则见 [ADR-0043](adr/0043-bundled-script-runtimes.md)。

实际发布验收采用 Python 3.12.16、Node 22.23.5 与 ad-hoc 签名 tar 包。移动解开后的 Python 加载包内 CBOR 扩展和 SSL，Node 加载原生 CBOR 模块并执行动态代码，两组 KV 服务完成读写并有序退出。[运行记录](measurements/m21-script-runtimes.json)保存实际版本、路径、签名与进程结果。

## WIT 接口生成

`plugins/kv-api/kv.wit` 定义 kv 接口、迁移状态与事件。修改 WIT 后，通过开发工具生成各语言绑定：

```bash
cargo run -p cordis-dev -- idl gen --lang rust \
  --output plugins/kv-api/src/lib.rs plugins/kv-api/kv.wit
cargo run -p cordis-dev -- idl gen --lang python \
  --output target/kv.py plugins/kv-api/kv.wit
cargo run -p cordis-dev -- idl gen --lang ts \
  --output target/kv.ts plugins/kv-api/kv.wit
```

生成结果包括提供方接口、数据类型与调用方代理。Rust 接口保留类型化调用，并可通过通用调用传递复合资源；Python 与 TypeScript 绑定使用同一套 CBOR 表示。WIT 的类型映射、方法顺序、借用资源与异步值规则见 [ADR-0039](adr/0039-wit-bindings.md)。

## Python 进程插件

在现有项目中运行 `cordis new-plugin hello --python`，开发工具会生成插件代码、当前 SDK 和依赖声明，将实例加入配置，并登记源目录的 watch 监视。安装该插件的依赖后即可运行：

```bash
conda run -n base python -m pip install \
  --target plugins/hello/.dependencies \
  -r plugins/hello/requirements.txt
```

生成的插件使用部署环境中的 Python，标准输出传输 UP/1；日志通过 `Context.log` 交给核心。Rust 与 Python 插件可在同一项目中编写，Python 源目录已加入 Cargo workspace 的排除列表。示例 `plugins/kv-py` 和 `plugins/counter-py` 分别采用逻辑名 `kv`、`counter`，与原生实现共用接口与状态格式，支持跨插口替换和失败回退。

Python SDK 的服务、效应、状态、隔离域、拦截视图、子组件与事件资源接口见 [Python SDK](../sdks/python/README.md)。运行依赖为 `cbor2==5.6.5`；项目提供方可以通过生成的 WIT 绑定与其他语言调用。

## TypeScript 进程插件

TypeScript 插件运行于 Node 22 及以上版本，通过 UP/1 与核心通信。在现有 Cordis 项目中创建并编译插件：

```bash
cordis new-plugin greeter --ts
npm --prefix plugins/greeter install
npm --prefix plugins/greeter run build
cargo build --target-dir target
cordis run
```

脚手架将 SDK 源码保存于 `plugins/greeter/src/cordis_sdk`，业务入口为 `src/main.ts`，构建结果进入 `dist/`。清单启动 `node dist/main.js`。`watch` 监视插件目录，重新编译后将模块变化交给管理事务；源目录同时登记到 Cargo workspace 的排除列表。

插件依赖 `cbor-x 1.6.0`，TypeScript 5.9.3 和 Node 类型声明参与编译。生成的 npm 配置使用普通文件安装依赖，构建脚本直接调用 TypeScript 编译器，供核心固定完整插件版本副本。

`cordis idl gen --lang ts` 生成提供方接口、值类型和 Client。使用方通过 `await context.get("kv@1.0", binding, binding.KvClient)` 注入服务；提供方通过 `context.provide` 登记实现。`context.effect` 与 `defer` 管理资源寿命，`exportState`、`Frozen` 和 `inherited` 处理状态；`isolate`、`intercept`、`plugin` 支持跨进程组合。调用取消通过 `withSignal` 传递，提供方以 `currentSignal` 接入异步操作。完整接口与资源释放示例见 [TypeScript SDK](../sdks/typescript/cordis-sdk/README.md)。

仓库中的 `plugins/kv-ts` 与原生、Python KV 共用逻辑名 `kv`、接口 `kv@1.0` 和状态模式 `kv.snapshot@1`。先构建共享 SDK，再构建示例插件：

```bash
npm --prefix sdks/typescript/cordis-sdk ci --ignore-scripts
npm --prefix sdks/typescript/cordis-sdk run build
npm --prefix plugins/kv-ts ci --ignore-scripts
npm --prefix plugins/kv-ts run build
```

M19 的双向调用、8 项原生 KV 契约、私有域和命名域、资源与事件转交、跨插口迁移及失败回退均通过真实核心验收。两次完整工作区测试各通过 311 条用例，20 类测试现场的文件数、字节数和目录数保持原值，详见 [M19 SDK 验收](measurements/m19-typescript-sdk.json)与[测试现场验收](measurements/m19-test-cleanup.json)。

## 网页插件与全栈插件

网页模块通过 `cordis-web` 执行组件入口，并经 HTTP 插件转发的 WebSocket 使用 UP/1。组件的依赖、效应和页面子实例由核心内核管理。每页拥有自己的服务域；页面可以注入 Rust、Python 和 TypeScript 后端提供的服务。

在已有 Cordis 项目中，可以分别生成纯网页与三种后端组合：

```bash
cordis new-plugin notes --web
cordis new-plugin native-panel --rust --web
cordis new-plugin python-panel --python --web
cordis new-plugin node-panel --ts --web
```

各插件的 README 给出具体构建方法。Rust 组合模板使用 `sh plugins/native-panel/build.sh`，把 Cargo 的动态库产物复制进插件目录；Python 和 TypeScript 组合沿用前两节的依赖安装与编译方式。纯网页入口是普通 ES 模块，文件位于 `plugins/notes/web/index.js`。

网页发布需要 HTTP 插件提供 `cordis.web@1.1`。将已构建的 HTTP 动态库放入项目的 `runtime.plugin_dir`，并在配置中登记：

```toml
[instance.http]
plugin = "http"
component = "http"
forwarder = true
```

HTTP 默认首次绑定本机动态端口，通过 Web 服务公布页面入口。窗口插件可以直接使用这个入口；浏览器示例的配置使用明确端口，运行方式见[网页示例](../examples/web/README.md)。示例包括页面笔记、Rust KV 全栈面板和 Python KV 全栈面板，提供字节读写、键列表与独立页面计数。

清单中 `assets` 相对插件目录，`entry` 相对资源目录：

```toml
name = "notes"
version = "0.1.0"
socket = "web"

[web]
assets = "web"
mount = "/notes"
entry = "index.js"
```

浏览器入口默认导出 `defineComponent` 的组件定义。模板将 DOM 与事件监听器登记为效应，计数通过 `exportState` 导出，并在写入前检查 `Frozen`。替换时核心导出各页状态、撤回旧界面，再让网页宿主导入带新序号的模块地址；页面对象和整页导航保持原状。全栈插件先完成后端激活，再更新所有页面，任一步失败时恢复原后端与原页面。

`watch` 将本插件目录的文件变化作为替换来源。多个插件在同一防抖窗口变化时，`replace_batch` 统一决定提交或恢复。页面关闭回收本页子实例；移除插件撤回它的资源路由与各页界面。网页服务之间的共享限定在同一页，跨页面数据由后端服务持有。

## 里程碑验收摘要（M0–M18）

第一阶段 `M0`–`M6` 交付内核，第二阶段推进独立核心、插件宿主与开发工具，每个里程碑一个提交。

| 里程碑 | 内容 |
|---|---|
| `M0` | 工程骨架 |
| `M1` | 内核基座与同步效应 |
| `M2` | 事件 |
| `M3` | 组件生命周期 |
| `M4` | 反应式余效应 |
| `M5` | 拦截、调用方句柄、日志、替换 |
| `M6` | 收尾：示例、多线程压力测试、接口冻结 |
| `M7`–`M9` | 拥有型名称、ABI、Rust SDK 与通用调用 |
| `M10` | UP/1 与唯一核心的进程监管 |
| `M11` | 原生插件宿主、组内替换、隔离域与组合；ABI v1.0 冻结 |
| `M12` | 核心可执行文件、声明式部署、管理接口、watch 与脚手架 |
| `M13` | 独立进程中的契约验证与验证印记 |
| `M14` | 系统验收、替换指标与使用文档 |
| `M15` | 插件编写 skill、共享模板与独立新会话验收 |
| `M16` | 发布压缩包与 `.app`、可移动验证副本、HTTP 页面入口与独立窗口进程 |
| `M17` | WIT 三语言接口生成、复合资源编解码、共享编译目录 |
| `M18` | Python SDK、KV 与计数插件、Python 脚手架、跨插口双向替换 |

`M0`–`M15` 的仓库交付及评审修复已完成。插件编写手册经过独立新会话的热替换、周期检查点与 KV 契约验证，ABI v1.0 已冻结。M12 的三个产品解析目标均完成固定 1800 秒 fuzz，正式源码与被测源码已逐字节比较。M14 的固定 50 次替换、直接调用、隔离与核心恢复结果见 [运行时验收与测量](measurements/m14-acceptance.md)。

M16 的实现与阶段检查已完成。Apple Silicon macOS 上的五项发布打包测试通过，覆盖运行产物交付、签名后契约重验、包外状态和多验证根；窗口过程验收记录 875 ms 恢复及依赖方重建。工作区覆盖 299 条运行测试与 2 条文档测试，固定 20 次迁移验收全部通过。Codex 用户目录的 skill 链接已按授权安装。

M18 已通过阶段检查：Python SDK 与原生插件双向调用、8 项 KV 契约、资源与事件转交、隔离域和跨插口双向替换均通过；工作区共 306 条用例通过。`new-plugin --python` 生成的插件与 Rust 成员共同构建运行，watch 可替换修改后的 Python 源码。M17 的 WIT 三语言绑定及 ABI v1.0 保持有效，AU-17 已记录截图与 Cmd-Q 的完整成功结果。原始测量与阶段检查见 [PROGRESS](PROGRESS.md)，下一步为 M19 TypeScript SDK。
