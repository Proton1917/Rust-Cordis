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
10. [动态调用](#动态调用)
11. [Python 进程插件](#python-进程插件)
12. [TypeScript 进程插件](#typescript-进程插件)
13. [网页插件与全栈插件](#网页插件与全栈插件)
14. [HTTP 路由插件](#http-路由插件)
15. [通用字节流与调用身份](#通用字节流与调用身份)
16. [密钥与服务环境](#密钥与服务环境)
17. [里程碑验收摘要（M0–M18）](#里程碑验收摘要m0m18)

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
CARGO_ENCODED_RUSTFLAGS="$(conda run -n base python tools/build_paths.py)" \
  cargo build --release --target-dir target/shared \
  -p cordis-runtime -p cordis-host -p cordis-dev -p kv-mem
mkdir -p target/pack-demo/plugins
cp target/shared/release/libkv_mem.dylib target/pack-demo/plugins/
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
target/shared/release/cordis pack \
  --project target/pack-demo \
  --target aarch64-apple-darwin \
  --runtime-dir "$PWD/target/shared/release" \
  --out dist/
```

产物为 `target/pack-demo/dist/cordis-demo-0.1.0-aarch64-apple-darwin.tar.gz`。解开后包含核心、宿主、插件、部署配置和 `service/install.sh`；在解开的目录执行 `./cordis run` 即可启动。Linux 使用 `.so` 库与 `x86_64-unknown-linux-gnu` 目标。有 Cargo 清单的插件项目会按指定 target 构建 release，工具从 Cargo 结构化产物记录取得库和可执行文件。

macOS 桌面部署可继续加入 HTTP 和窗口插件：

```bash
CARGO_ENCODED_RUSTFLAGS="$(conda run -n base python tools/build_paths.py)" \
  cargo build --release --target-dir target/shared -p cordis-http -p cordis-window
mkdir -p target/pack-demo/plugins/window
cp target/shared/release/libcordis_http.dylib target/pack-demo/plugins/
cp target/shared/release/cordis-window plugins/window/cordis-plugin.toml \
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
target/shared/release/cordis pack \
  --project target/pack-demo \
  --target aarch64-apple-darwin \
  --runtime-dir "$PWD/target/shared/release" \
  --app --out applications/
```

生成的 `target/pack-demo/applications/cordis-demo.app` 从自身位置读取部署配置，窗口通过 `cordis.web@1.0` 取得 HTTP 启动页。HTTP 的 `assets` 可指定静态目录；同时把该相对路径加入 `pack.assets`，页面资源便随发布包交付。状态默认保存到用户 `Library/Application Support/org.cordis.cordis-demo`，也可以通过 `pack.state_dir` 指定包外绝对目录；安装的新插件和宿主缓存分别位于该状态目录的 `plugins/` 与 `.cordis/lib/`。

`--sign` 指定本机签名身份并启用 hardened runtime，`--allow-third-party` 为宿主设置第三方库装载权限。启用 `require_verified` 的部署在签名改变库字节后重新执行原契约套件，验证副本及真实结果一起发布。配置、签名及可移动验证副本的规则见 [ADR-0036](adr/0036-release-packaging.md)。

`pack --target-dir <目录>` 指定复用的 Cargo 编译目录，缺省为部署项目的 `target/`；相对目录以部署项目为基准。

### 构建路径隐私

cordis-dev 0.2.1 的 pack 对 Cargo 构建的原生库与进程程序默认映射 HOME、CARGO_HOME、Rust sysroot 和项目目录。用户的 CARGO_ENCODED_RUSTFLAGS 或 RUSTFLAGS 保留在默认参数之后；规则与官方发布共用 tools/path-remapping.json。macOS 动态库在编译期采用相对安装名称。

核心和宿主从 `--runtime-dir` 复制，交付前与其他 Mach-O、ELF 及 TOML、JSON、plist 清单一起接受完整字节检查。检测到当前 HOME 或用户名时，打包返回错误，并列出相对文件名及命中次数。按以下命令从本仓库构建可交付的核心、宿主及官方插件，再将产物目录传给 `--runtime-dir`：

```bash
CARGO_ENCODED_RUSTFLAGS="$(conda run -n base python tools/build_paths.py)" \
  cargo build --release --target-dir target/shared \
  -p cordis-runtime -p cordis-host -p cordis-dev \
  -p cordis-http -p cordis-window -p cordis-secrets
```

应用清单应使用可移植的相对路径。明确接受路径披露时，可传入 `--allow-private-paths`，保留警告并继续生成发布包。新 watch 0.1.1 使用 cordis.runtime@1.4，配套核心为 0.1.1；已有 1.3 读取方继续兼容。详细机制见 [ADR-0049](adr/0049-page-projections-and-package-paths.md)。

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

## 动态调用

方法名称来自配置、配方或网关映射时，可以通过动态代理调用已声明的服务。Rust 的 `Context::get_dynamic` 与类型化 `get` 共用依赖视图：提供方替换时使用方重建，失败回滚后继续取得原实现，提供方卸下后使用方转为 Pending。Scoped 提供方收到相同的业务元数据与 `cordis.caller`。

普通插件服务在 inject 中声明，包括 `cordis.http`、`cordis.secrets` 和 `cordis.byte-stream`。核心自己提供的内置服务沿用类型化 get 的既有取得规则。Rust 可以按接口类型声明依赖，也可以使用 `Inject::with_name("kv@1.0")`；可选声明为 `optional_name`，名称与版本沿同一装载校验处理。

```rust
use cordis_sdk::prelude::*;

async fn write_message(context: &Context) -> Result<Option<Bytes>> {
    let service = context.get_dynamic("kv@1.0")?;
    service.invoke_as::<_, ()>("put", &("message", Bytes(b"hello".to_vec())))?;
    let value = service.invoke_as("get", &("message",))?;
    service.invoke_async("flush", CborValue::Array(Vec::new())).await?;
    Ok(value)
}
```

参数使用位置数组。Rust 单参数 tuple 写成 `(value,)`，零参数使用空 `Vec<CborValue>` 或 `CborValue::Array(Vec::new())`。`invoke`/`invoke_as` 使用同步入口，`invoke_async`/`invoke_as_async` 使用异步入口；异步入口也能承接同步方法。`Bytes` 表示字节串，继承 v0.x 的字节串与旧数组双读规则。

原始 `invoke` 与 `invoke_async` 接收、返回普通 CBOR 数据。参数出现资源标签时返回参数错误；结果出现资源标签时，先归还结果的资源引用，再返回提示使用 `invoke_as` 的错误。含服务对象的参数和结果使用 Serde 的 `Service` 字段，由 `invoke_as` 或 `invoke_as_async` 完成资源转交。例如提供方的 open 方法返回通用字节流时：

```rust
use cordis_sdk::{CborValue, DynamicService, Result, Service};
use stream_api::ByteStream;

async fn open_stream(service: &DynamicService) -> Result<Service<dyn ByteStream>> {
    service.invoke_as_async("open", &Vec::<CborValue>::new()).await
}
```

Python 通过现有的 `get` 取得 `Remote`，按方法名调用。`binding` 是 `cordis idl gen --lang python` 生成的模块：

```python
async def read_message(context, binding):
    service = await context.get("kv@1.0", binding)
    try:
        return await service.invoke("kv@1.0", "get", ["message"])
    finally:
        service.close()
```

TypeScript 的 `getDynamic` 返回同一个拥有资源引用的 `Remote`。`binding` 使用生成模块，方法参数按普通 CBOR 数据类型提供：

```typescript
import {Context, Binding} from "cordis-sdk";

async function readMessage(context: Context, binding: Binding): Promise<unknown> {
    const service = await context.getDynamic("kv@1.0", binding);
    try {
        return await service.invoke("kv@1.0", "get", ["message"]);
    } finally {
        service.close();
    }
}
```

脚本 SDK 的资源标签携带拥有关系，生成绑定负责导入与导出；裸资源编号在发送边界返回参数错误。服务句柄由当前操作或效应持有，操作结束或组件清理时归还。

类型化代理提供编译期方法名与参数检查，并在同组原生调用中使用直接 ABI 方法槽。动态代理统一使用 V-2 通用入口，按方法名分派并进行 CBOR 编解码，适用于方法名称确实由运行时输入决定的调用；未知方法和参数错误在调用时返回。

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

### 自定义页面外壳

正式应用应提供自己的外壳。HTTP 实例的 `assets` 指向应用页面目录，`index` 选择该目录中的入口文件。下面的 `web/index.html` 为页面组件保留挂载点，并建立网页宿主连接：

```html
<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>应用工作台</title>
  <script src="/_cordis/session.js"></script>
</head>
<body>
  <header>应用工作台</header>
  <main id="cordis-components"></main>
  <script type="module">
    import {connect} from '/_cordis/cordis-web.js';
    connect({token: globalThis.CORDIS_SESSION.token});
  </script>
</body>
</html>
```

```toml
[instance.http.config]
listen = "127.0.0.1:8080"
assets = "web"
index = "index.html"
```

组件替换回退自身登记的 DOM，页面外壳和整页导航状态继续保留。页面源码中的协议路径与挂载标识属于协议，用户可见文本采用应用自己的名称。默认开发页的标题来自 `http.title`，缺省为空，此时浏览器使用地址；正常连接时隐藏状态提示，中断或结束时显示中性提示。

## HTTP 路由插件

体系外的 REST 客户端、上传下载和支付回调通过 HTTP 路由接入。处理方注入 `cordis.http@1.0`，把 `HttpHandler` 对象交给 `route`；登记和异步 `unroute` 归属组件效应。生成处理方时选择语言：

```sh
cordis new-plugin api --rust --http
cordis new-plugin webhook --python --http
cordis new-plugin files --ts --http
```

部署中同时登记 HTTP 提供方与处理方。以下配置使用原生回显示例，并允许 100 MiB 上传：

```toml
[runtime]
plugin_dir = "plugins"

[instance.http]
plugin = "libcordis_http.dylib"
component = "http"
forwarder = true
config = { listen = "127.0.0.1:8080", max_body_bytes = 104857600 }

[instance.api]
plugin = "libhttp_echo.dylib"
component = "http-echo"
config = { prefix = "/api" }
```

Rust、Python 与 TypeScript 的示例分别位于 `plugins/http-echo`、`plugins/http-echo-py` 和 `plugins/http-echo-ts`。普通请求回显方法、原编码路径、命中前缀、查询串、请求头、对端与正文；`PUT /api/file` 保存上传，`GET /api/file` 分块下载，`DELETE /api/file` 删除保存的数据。

前缀按路径段选择最长者，例如 `/api` 匹配 `/api` 与 `/api/items`。同名路由、协议地址和网页挂载的重叠登记返回冲突错误。方法集合外的请求得到 405 与 Allow；处理错误、超时、处理方退出和非法响应状态分别由 HTTP 提供方转换为固定状态及中性正文。

| HTTP 配置 | 缺省值 | 用途 |
|---|---:|---|
| inline_body_bytes | 1048576 | 已知小请求使用内联字节串，最大 4 MiB |
| max_body_bytes | 16777216 | 请求体总量上限；单路由可进一步收紧 |
| stream_chunk_bytes | 1048576 | 流读取的单块上限，最大 4 MiB |
| route_timeout_ms | 30000 | 等待处理方响应头的截止时间 |
| body_idle_ms | 30000 | 流读取的空闲截止时间 |
| replace_grace_ms | 2000 | 撤回前缀后等待新处理方的宽限 |

HTTP 过滤逐跳头，其余端到端字段按原值转交，重复 Set-Cookie 保持顺序。处理方负责认证与转发头的信任策略。浏览器组件继续经 UP/1 注入后端服务；登录和退出由应用路由完成后，通过页面导航建立带最新 Cookie 的连接。

### 反向代理

TLS 由反向代理终结。HTTP 保持本机监听，并把浏览器实际使用的外部 origin 放入 `public_origins`：

```toml
[instance.http.config]
listen = "127.0.0.1:8080"
public_origins = ["https://app.example.com"]
max_body_bytes = 104857600
```

Nginx 的同源代理配置可采用：

```nginx
map $http_upgrade $connection_upgrade {
    default upgrade;
    '' close;
}
server {
    listen 443 ssl;
    server_name app.example.com;
    ssl_certificate /etc/nginx/certs/app.pem;
    ssl_certificate_key /etc/nginx/certs/app.key;
    client_max_body_size 100m;
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_request_buffering off;
        proxy_buffering off;
    }
}
```

页面、WebSocket 与 `/api` 共用该域名。HTTP 握手接受本次监听地址的 origin 以及 `public_origins`，会话令牌继续保护页面连接和入口清单。

## 通用字节流与调用身份

`cordis.byte-stream@1.0` 把数据作为拥有型服务对象传递。接收方调用 `read(max_bytes)` 拉取，读到空值结束；读取上限接受 1 字节至 4 MiB。Rust 的 `stream_api::from_file`、Python 的 `cordis_sdk.streams.from_file`、TypeScript 的 `fromFile` 创建文件流；相应 `to_file`、`streamToFile` 按块写入。迭代器与回调适配分别为 `from_chunks`/`fromChunks` 和 `copy_to`/`copyStream`。

WIT 的 `list<u8>` 在方法、记录、变体和容器中统一编码为 CBOR 字节串，读取同时兼容旧整数数组。Rust 聚合字段使用 `cordis_sdk::Bytes`，可通过 `Vec<u8>.into()` 构造；Python 使用 `bytes`，TypeScript 使用 `Uint8Array`。已有 v0.x 的 KV 状态继续按相同模式与版本继承。

Rust 插件可以在自己拥有的线程或 Tokio 运行时中调用 SDK。宿主接管跨进程调用、计时器和生命周期操作的执行；丢弃普通调用 Future 会向远端传播取消，已受理的组件装入和效应清理继续完成。插件通过效应清理等待自有线程退出，并在退出前释放服务与资源引用。

Scoped 提供方通过三种 SDK 的 `caller(metadata)` 取得直接调用方：后端为 `{ instance }`，页面为 `{ page_of }`。服务页面的插件按 `page_of` 显式授权。后端处理页面请求期间继续使用自己的实例身份调用其他服务。

`cordis.page` 提供 HTTP 实例名和不透明页面键。HTTP 持有连接升级时的 `page_headers` 快照与直接对端，缺省选择 Cookie、User-Agent、Accept-Language，总量上限为 8 KiB。页面关闭后键失效；登录或退出成功后进行页面导航，以新连接更新快照。原生 `http_api::page_attributes`、Python `page_attributes` 和 TypeScript `pageAttributes` 辅助查询当前依赖域内的 HTTP 属性。

## 密钥与服务环境

密钥服务由 `secrets` 插件提供。部署系统写入 NAME=VALUE 文件，属主为运行用户、权限至多 0600；包含空白或井号的值使用引号。插件按自己的 grants 配置授权后端实例：

```toml
[instance.secrets]
plugin = "libcordis_secrets.dylib"
component = "secrets"
config = { file = "private/credentials.env", grants = { api = ["PAYMENT_SIGNING_KEY"] } }
```

处理方声明 `cordis.secrets@1.0` 依赖，通过 `get` 读取获准的名字。页面身份得到权限错误；`cordis.` 元数据由核心建立。密钥的读取、解释与保存归插件，跨进程值以不透明载荷转交。`cordis pack` 按密钥服务的实际声明排除配置引用的密钥文件，安装环境另外提供该文件。

PATH、LANG 等普通变量可交给服务管理器：

```sh
cordis service install --name com.example.app \
  --output service/app.plist \
  --env PATH=/opt/homebrew/bin:/usr/bin:/bin \
  --inherit-env LANG
```

变量名含 SECRET、TOKEN、PASSWORD 或 KEY 时返回错误，并提示使用密钥插件。显式值优先于继承值；`--output` 在指定位置生成服务配置。

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

M18 阶段检查覆盖 Python SDK 与原生插件双向调用、8 项 KV 契约、资源与事件转交、隔离域和跨插口双向替换，当时工作区共 306 条用例通过。`new-plugin --python` 生成的插件与 Rust 成员共同构建运行，watch 可替换修改后的 Python 源码。M17 的 WIT 三语言绑定及 ABI v1.0 保持有效，AU-17 已记录截图与 Cmd-Q 的完整成功结果。各里程碑的测量与检查结果见 [PROGRESS](PROGRESS.md)。
