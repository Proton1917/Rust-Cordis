# 13 第二阶段架构：crate 划分、内核改动与运行时

| 项 | 内容 |
|---|---|
| 文档编号 | `DOC-13` |
| 状态 | 已定稿（2026-11-11）。接口草图（§4）的签名在实现中可调整，调整后同步更新本文 |
| 读者 | 实现者、评审者、插件作者 |
| 输入 | `DOC-10`–`DOC-12`；第一阶段内核源码（提交 `e459aa7`） |
| 输出 | crate 布局与规模上限、内核改动 `K-1`–`K-5`、各 crate 职责、SDK 接口草图、运行时结构、架构规则 `B-1`–`B-10`、依赖白名单 |
| 不包含 | ABI 布局（`DOC-12`）、里程碑（`DOC-14`） |

## 1 crate 布局

```
crates/
  cordis/               内核（第一阶段）。M7 做 K-1–K-5 的改动
  cordis-testkit/       确定性执行器（第一阶段）
  cordis-abi/           ABI v1 类型、宿主侧与插件侧的安全包装、布局测试
  cordis-sdk/           插件作者依赖的 crate：Component、Context、Service、事件、状态
  cordis-sdk-macros/    #[interface]、#[event]、export_plugin!
  cordis-loader/        装载器：打开库、握手、适配到内核、租约、热替换、回滚
  cordis-runtime/       核心可执行文件 cordis：tokio 宿主、配置、目录监视、状态目录、verify 子命令
  cordis-contract/      契约套件的编写辅助与 cordis.contract@1 接口
plugins/                示例插件（每个都是独立的 cdylib crate）
  kv-api/               接口 crate：kv@1
  kv-mem/               kv@1 的内存实现
  kv-file/              kv@1 的文件实现
  kv-contract/          kv@1 的契约套件
  counter/              注入 kv@1 的使用方，导出状态
  console-log/          把日志输出到终端的插件
fixtures/               测试用插件库（v1/v2、故障注入、ABI 违规），只供测试使用
docs/abi/v1-layout.txt  ABI 布局记录（由测试生成）
```

依赖方向：

```
cordis-runtime ─→ cordis-loader ─→ cordis（内核）
       │                 └──────→ cordis-abi
       └─────────────────────────→ cordis-abi
plugins/*、fixtures/* ─→ cordis-sdk ─→ cordis-abi
                         cordis-sdk ─→ cordis-sdk-macros
```

`cordis-sdk` 与插件不依赖内核 crate。

### 1.1 规模上限

| crate | 目标行数 | 上限 |
|---|---:|---:|
| `cordis`（内核，含 `K-*` 改动） | 4800 | 6000（第一阶段上限不变） |
| `cordis-abi` | 1500 | 2500 |
| `cordis-sdk` | 2000 | 3500 |
| `cordis-sdk-macros` | 1200 | 2000 |
| `cordis-loader` | 2000 | 3500 |
| `cordis-runtime` | 1500 | 2500 |
| `cordis-contract` | 500 | 1000 |

超出目标时在 `docs/PROGRESS.md` 记录原因；超出上限需要写 ADR。

## 2 内核改动

内核只做下列改动。每项都有第一阶段之后的测试覆盖，第一阶段的全部测试继续通过。

| 编号 | 改动 | 原因 | 验证 |
|---|---|---|---|
| `K-1` | 组件名、服务名、事件名由 `&'static str` 改为宿主拥有的字符串（`Arc<str>`）。涉及 `ComponentDef.name`、`State.names`、日志名称等 | 插件的名字位于插件库内存中。旧库静止后不得再有指向它的引用 | 测试：用运行时构造的字符串创建组件定义与服务，原字符串释放后名称仍可读取 |
| `K-2` | 命名服务键：`ctx.provide_named(name, value)`、`ctx.get_named(name)`、`Inject::with_named(name)`，值类型为 `Arc<dyn Any + Send + Sync>`。类型化键 `ServiceKey` 以 `K::NAME` 映射到同一命名空间 | 单独编译的插件之间不能用 `TypeId` 识别服务（`ADR-0011`） | 测试：命名键与类型化键的提供、注入、替换、隔离域、拦截与第一阶段语义一致 |
| `K-3` | 命名事件：`ctx.on_named(name, listener)`、`ctx.emit_named(name, payload)`、`ctx.parallel_named`，载荷为 `Arc<dyn Any + Send + Sync>` | 同 `K-2` | 测试：命名事件的派发、监听器回退、重入语义与类型化事件一致 |
| `K-4` | `Registry::replace` 之后，内核不再持有旧 `ComponentDef` 的闭包。别名表只保存 `ComponentId → ComponentId` | 旧定义的闭包指向旧库代码，持有它会让旧库的租约永不归零。现状：`State.aliases: HashMap<ComponentId, ComponentDef>`（`crates/cordis/src/kernel.rs:69`），连续替换 A→B→C 后 B 的定义仍被别名表持有 | 测试：替换后用 `Weak` 观察旧定义捕获的值已释放；连续三次替换后同样成立 |
| `K-5` | 调和循环的复位守卫 | 已在 `940650c` 完成 | 该提交附带的测试继续保留 |

命名键与类型化键共用 `DOC-01` 的全部条款。`K-2`、`K-3` 是在现有键表上的另一种入口，不复制余效应逻辑。

服务匹配以名称为身份，提供登记表另行保存首次登记的方式与类型。依赖、读取、隔离及通知共同解析该名称；类型化读取在值转换边界检验其 `Arc<K::Value>` 表示。命名元数据通过 `Inject::with_named_meta` 声明，`Context::intercept_named` 增加后层优先的覆盖，`named_meta` 在定位声明后按视图层次解析。类型化元数据继续采用其 `Merge` 实现。事件保留第一阶段类型身份，命名事件以名称及同步/异步派发族识别；同名的两个派发族分别保留监听快照。

## 3 各 crate 职责

### 3.1 `cordis-abi`

- 定义 `DOC-12` 的全部类型。
- 提供插件侧的安全包装：把 Rust 闭包、`Future`、trait 对象包装成 `AbiObject`/`AbiFuture` 的泛型蹦床函数；panic 捕获；`AbiBuf` 的分配与释放。
- 提供宿主侧的安全包装：把收到的 `AbiObject`/`AbiFuture` 包装成持有租约的 Rust 值，`Drop` 时调用对方的 `drop` 并归还租约。
- 布局测试：对每个类型输出大小、对齐、字段偏移到 `docs/abi/v1-layout.txt`，测试比较生成结果与文件。
- 运行时依赖为 `serde`、`ciborium`；源码审计使用开发依赖 `syn`、`proc-macro2`（`ADR-0015`）。

### 3.2 `cordis-sdk`

插件作者唯一需要依赖的 crate。接口形状尽量与内核一致，使第一阶段写的组件只需少量修改就能编译为插件（§4）。

`cordis-sdk`、`cordis-sdk-macros`、`cordis-abi` 以及示例插件的 `rust-version` 设为 `1.85`（edition 2024 的最低版本），不随工作区的 1.96 走。理由：插件作者可能使用较旧的编译器（`G7`），SDK 的最低支持版本决定了这一点能否成立。`AC-8` 以 1.85.0 实际验证。

### 3.3 `cordis-sdk-macros`

| 宏 | 输入 | 生成 |
|---|---|---|
| `#[interface(name = "kv", version = "1.0")]` | 接口 trait | 方法虚表（`DOC-12` §7.2）、提供方蹦床、使用方代理类型、`Interface` 实现（名称、版本、方法数） |
| `#[event(name = "message", version = "1.0")]` | 事件结构体（serde） | `Event` 实现 |
| `export_plugin!(name, version, components = [...])` | 组件类型列表 | `CORDIS_PLUGIN_ABI`、`cordis_plugin_v1`、静态描述符 |

宏生成的 `unsafe` 使用固定模板：调用 `cordis-abi` 公开的 unsafe 函数、按接口声明调用 C 函数指针、实现 C 蹦床、导出符号属性以及持有静态 ABI 表的 Send/Sync 实现。模板的 SAFETY 依据同时保存在宏源码与生成的文档注释中（规则 `B-2`）。

### 3.4 `cordis-loader`

| 模块 | 职责 |
|---|---|
| `library` | 复制到唯一路径（`SW-1`）、`libloading` 打开、握手、描述符校验 |
| `lease` | `LibraryId` 分配、租约计数、静止判定 |
| `adapt` | 把描述符中的组件转换为内核 `ComponentDef`：配置以 CBOR 字节保存为 `RawConfig`；`apply` 调用插件函数并把返回的 `AbiFuture` 包装为内核 `Task`；宿主虚表的每个函数转发到内核的对应接口 |
| `swap` | `DOC-11` §6 的 `SW-1`–`SW-9` 与 §7 回滚 |
| `state` | 导出器登记表、状态块的保存与继承 |

`cordis-loader` 本身以组件的形式挂载在内核上：它登记的库、实例、监视都是效应，释放它即卸下全部插件。

### 3.5 `cordis-runtime`

- 可执行文件 `cordis`，子命令 `run`（默认）与 `verify`。
- tokio 多线程运行时实现 `Spawn + Clock`。
- 配置：`cordis.toml`（`DOC-11` §10）。配置解析失败时启动失败；运行中配置文件变为无效时保持当前状态并记录错误。
- 目录监视：`notify` crate，事件去抖 300 毫秒后计算差异。
- 状态目录布局：`state/<实例名>.state`、`state/consumed/`、`state/data/<插件名>/`（`cordis.data-dir@1`）、`state/status.json`。
- `status.json`：每个库的插件名、版本、副本序号和路径、是否验证、租约数、静止状态；每个实例的状态、纪元、最近一次替换报告。
- 信号：`SIGTERM`/`SIGINT` 触发 `DOC-11` §9 的有序退出。

### 3.6 `cordis-contract`

- 定义内置接口 `cordis.contract@1`：`report(name, passed, message)`、`finish()`。
- 提供断言辅助，套件作者用它编写套件插件。
- `cordis verify` 的子进程由 `cordis-runtime` 实现，结果判定由本 crate 的类型描述。

## 4 SDK 接口草图

```rust
// ── 接口 crate kv-api ──────────────────────────────────────────────
use cordis_sdk::prelude::*;

#[cordis_sdk::interface(name = "kv", version = "1.0")]
pub trait Kv: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;
    fn keys(&self) -> Result<Vec<String>>;          // Vec<String> 经 CBOR 编码
    async fn flush(&self) -> Result<()>;
}

// ── 插件 crate kv-file（crate-type = ["cdylib"]）──────────────────────
use cordis_sdk::prelude::*;
use kv_api::Kv;

#[derive(Clone, serde::Deserialize)]
struct KvConfig { path: String }

struct KvStore;
impl Component for KvStore {
    const NAME: &'static str = "kv-store";
    type Config = KvConfig;
    fn inject() -> Inject { Inject::new() }
    fn provides() -> Provides { Provides::new().with::<dyn Kv>() }
    async fn apply(ctx: Context, config: KvConfig) -> Result {
        let dir = ctx.data_dir()?;                                   // cordis.data-dir@1
        let store = Arc::new(FileStore::open(dir.join(&config.path))?);
        ctx.provide::<dyn Kv>(store.clone())?;
        ctx.effect("flush-on-stop", {
            let store = store.clone();
            move |fx| fx.defer_async(move || async move { store.flush().await })
        })?;
        Ok(())
    }
}

cordis_sdk::export_plugin!(name = "kv", version = env!("CARGO_PKG_VERSION"), components = [KvStore]);

// ── 使用方插件 counter ───────────────────────────────────────────────
struct Counter;
impl Component for Counter {
    const NAME: &'static str = "counter";
    type Config = CounterConfig;
    fn inject() -> Inject { Inject::new().with::<dyn Kv>() }        // 注入 kv@1.0
    async fn apply(ctx: Context, config: CounterConfig) -> Result {
        let kv = ctx.get::<dyn Kv>()?;                                // Service<dyn Kv>
        let start = match ctx.inherited_state() {
            Some(blob) if blob.schema() == "counter.state" => blob.decode::<u64>()?,
            _ => 0,
        };
        let count = Arc::new(AtomicU64::new(start));
        let frozen = ctx.export_state("counter.state", 1, {
            let count = count.clone();
            move || Ok(count.load(Ordering::SeqCst))
        })?;
        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                while ctx.is_current() && !frozen.is_set() {
                    let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                    kv.put("count", n.to_le_bytes().to_vec())?;
                    ctx.sleep(Duration::from_millis(config.interval_ms)).await;
                }
                Ok(())
            }
        })?;
        Ok(())
    }
}
```

## 5 运行时结构

### 5.1 进程内的组件树

```
根纤程
└── cordis-loader（组件）
    ├── 库 kv#1（效应：打开的库、租约表项）
    ├── 实例 store   = kv/kv-store    （内核纤程）
    └── 实例 counter = counter/counter     （内核纤程，注入 kv@1）
```

### 5.2 一次服务调用的路径

`counter` 调用 `kv.put(...)`：插件侧代理类型 → `kv-file` 库中虚表的 `put` 蹦床 → `FileStore::put`。调用不经过宿主，开销为一次间接调用加参数编码。宿主只参与对象的获取（`HostV1.get`）与租约管理。

### 5.3 内置接口

| 接口 | 提供方 | 内容 |
|---|---|---|
| `cordis.data-dir@1` | 运行时 | 返回实例所属插件的数据目录（`PR-7`） |
| `cordis.contract@1` | `cordis verify` 子进程 | 契约套件报告结果（`DOC-11` §8） |
| `cordis.runtime@1` | 运行时 | 只读查询：已装载库、实例状态（供管理类插件使用） |

## 6 架构规则

| 编号 | 规则 |
|---|---|
| `B-1` | 内核 crate 继续遵守第一阶段的 `A-1`–`A-12`，不使用 `unsafe`，不依赖 tokio |
| `B-2` | `unsafe` 位于 `cordis-abi`、`cordis-loader` 与宏生成的固定代码形态。每个 unsafe 块、函数、impl、extern 块和属性前一行有 `// SAFETY:` 注释，说明依赖的 `DOC-12` 条款；宏源码中的对应模板保存该注释并生成 SAFETY 文档注释。允许模板调用经长度检查的 C 函数指针、建立 ABI 对象、生成蹦床与导出符号。源码审计使用 syn 解析。`cordis-abi` 与 `cordis-loader` 开启 `#![deny(unsafe_op_in_unsafe_fn)]` |
| `B-3` | 跨插件边界只出现 `DOC-12` 的类型（`Q3`）。`cordis-sdk` 的公开接口中不出现内核类型 |
| `B-4` | 每个跨边界函数在被调用侧捕获 panic（`DOC-12` §6） |
| `B-5` | 宿主侧每个收到的对象都包装为持有租约的值（`DOC-12` §11） |
| `B-6` | 装载器的每一步副作用（打开的库、挂载的实例、监视器、状态块）都登记为内核效应 |
| `B-7` | `SW-1`–`SW-4` 保持实例、纪元和绑定原状；`SW-5` 仅进入可逆冻结，导出失败时解冻；`SW-6` 开始生命周期替换（`DOC-11` §6） |
| `B-8` | 持有内核锁或装载器内部锁时，不调用插件代码 |
| `B-9` | 插件代码的调用都发生在宿主执行器的任务中或调用方线程上，装载器不为插件创建隐藏线程 |
| `B-10` | `docs/abi/v1-layout.txt` 在 `M8` 冻结后只随 ADR 修改 |

## 7 依赖白名单

内核 crate 的白名单不变（`DOC-02` §9）。其余 crate 的运行时依赖取自下表，新增需要写 ADR。

| crate | 用途 | 使用者 |
|---|---|---|
| `serde` | 配置、DTO、状态块的序列化 | `cordis-abi`、`cordis-sdk`、`cordis-runtime` |
| `ciborium` | CBOR 编解码（`ADR-0010`） | `cordis-abi`、`cordis-sdk`、`cordis-loader` |
| `libloading` | 打开动态库 | `cordis-loader` |
| `tokio` | 宿主执行器与计时 | `cordis-runtime` |
| `notify` | 目录监视 | `cordis-runtime` |
| `toml` | 解析 `cordis.toml` | `cordis-runtime` |
| `syn`、`quote`、`proc-macro2` | 过程宏 | `cordis-sdk-macros` |
| `parking_lot`、`futures-util`、`thiserror` | 与内核相同 | 各 crate |

开发依赖不受此表限制，但需要在 `PROGRESS.md` 记录新增项。
