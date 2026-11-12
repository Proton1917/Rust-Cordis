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

| 路径 | 职责 |
|---|---|
| `crates/cordis` | 第一阶段内核及 K-1–K-5 |
| `crates/cordis-testkit` | 确定性执行器 |
| `crates/cordis-up` | UP/1 消息类型、受限 CBOR 编解码与帧协议 |
| `crates/cordis-types` | 内置接口的纯 serde 数据类型，MSRV 1.85（ADR-0028） |
| `crates/cordis-supervisor` | 核心侧进程监管、对象路由、截止时间、背压与故障回收 |
| `crates/cordis-process` | 共享的安全进程执行、组回收与受限探测；开发工具与监管器复用 |
| `crates/cordis-runtime` | 核心可执行文件 cordis：运行、状态、检查点、管理接口与服务安装 |
| `crates/cordis-dev` | 独立开发工具：脚手架、验证、打包与代码生成，通过 cordis-host 读取原生描述符 |
| `crates/cordis-host` | 原生插件宿主程序：分组装载、ABI 适配、租约、组内替换、UP/1 桥 |
| `crates/cordis-abi` | 原生插件与宿主之间的 ABI 类型、安全包装、布局测试 |
| `crates/cordis-sdk` | 原生插件作者接口：Component、Context、Service、事件、状态 |
| `crates/cordis-sdk-macros` | interface、event 与 export_plugin 宏 |
| `crates/cordis-contract` | 插件侧契约断言辅助，复用 cordis.contract@1 的 SDK 接口 |
| `plugins/kv-api` | kv@1 接口定义 |
| `plugins/kv-mem`、`plugins/kv-file` | kv@1 的内存和文件实现 |
| `plugins/kv-contract`、`plugins/counter`、`plugins/console-log` | 契约套件、计数使用方、终端日志 |
| `plugins/watch` | 持有管理授权的文件监视与自动替换插件 |
| `plugins/window` | M16 的独立进程窗口插件，主线程运行界面并提供 window@1 |
| `fixtures/` | 真实行为与故障测试插件 |
| `docs/abi/v1-layout.txt` | ABI 布局记录，由测试生成 |

依赖方向：

| 使用方 | 项目内产品依赖 |
|---|---|
| cordis-runtime | cordis、cordis-supervisor、cordis-up、cordis-types |
| cordis-supervisor | cordis、cordis-up、cordis-process、cordis-types |
| cordis-process | cordis-up |
| cordis-host | cordis-up、cordis-abi、cordis-types |
| cordis-dev | cordis-up、cordis-process、cordis-types；通过子进程使用 cordis、cordis-host |
| 原生插件、契约辅助与原生 fixture | cordis-sdk；接口 crate |
| cordis-sdk | cordis-abi、cordis-sdk-macros、cordis-types |

核心与插件宿主通过 UP/1 通信。Q0 要求核心进程的依赖树与 ABI、SDK 分离，内置接口以 UP/1 数据协议提供；宿主负责转换为原生对象（ADR-0017、ADR-0018）。

`cordis-sdk` 与插件不依赖内核 crate。

### 1.1 规模上限

| crate | 目标行数 | 上限 |
|---|---:|---:|
| `cordis`（内核，含 `K-*` 改动） | 4800 | 6000（第一阶段上限不变） |
| `cordis-abi` | 2500 | 3000（ADR-0022） |
| `cordis-sdk` | 2000 | 3500 |
| `cordis-sdk-macros` | 1200 | 2000 |
| `cordis-types` | 200 | 400 |
| `cordis-process` | 400 | 700（ADR-0029） |
| `cordis-up` | 1500 | 2500 |
| `cordis-supervisor` | 4500 | 5500（ADR-0030） |
| `cordis-host` | 2000 | 3500 |
| `cordis-runtime` | 4500 | 5500（ADR-0034） |
| `cordis-dev` | 1500 | 2500 |
| `cordis-contract` | 500 | 1000 |

目标与上限统计 `src/` 中的 Rust 代码行，排除空行、普通注释与文档注释；同一行同时有代码与注释时计为代码行。`docs/PROGRESS.md` 同时报告“代码行 / 总行”并记录统计工具与算法。

超过目标时在 `docs/PROGRESS.md` 记录原因。通过职责拆分、去重和复用调整实现，保持标准格式、必要注释与完整功能；调整后代码行仍超过上限时写 ADR，继续推进，由设计方复核。全部模块与关键函数使用文档注释说明负责步骤、前置条件和失败处理，并引用相应条款。约 60 行以上的函数按实际步骤拆分。

## 2 内核改动

内核只做下列改动。每项都有第一阶段之后的测试覆盖，第一阶段的全部测试继续通过。

| 编号 | 改动 | 原因 | 验证 |
|---|---|---|---|
| `K-1` | 组件名、服务名、事件名由 `&'static str` 改为宿主拥有的字符串（`Arc<str>`）。涉及 `ComponentDef.name`、`State.names`、日志名称等 | 插件的名字位于插件库内存中。旧库静止后不得再有指向它的引用 | 测试：用运行时构造的字符串创建组件定义与服务，原字符串释放后名称仍可读取 |
| `K-2` | 命名服务键：`ctx.provide_named(name, value)`、`ctx.get_named(name)`、`Inject::with_named(name)`，值类型为 `Arc<dyn Any + Send + Sync>`。`lookup_named(name: impl Into<Arc<str>>) -> Option<AnyArc>` 查询当前域中处于 Active 的实时绑定，get_named 读取声明依赖的纪元承诺绑定。类型化键 `ServiceKey` 以 `K::NAME` 映射到同一命名空间。M10 按 `ADR-0016` 增加依赖声明的 `required` 和可用性谓词 | 独立编译插件按名称识别服务；每个使用方分别声明可选性与最低次版本；M12 按最终候选图检查核心内置服务 | 测试：命名与类型化服务的提供、注入、替换、隔离、拦截；可选项纪元变化、使用方版本门槛及实际绑定清理顺序；整批新提供方与新使用方共同切换 |
| `K-3` | 命名事件：`ctx.on_named(name, listener)`、`ctx.emit_named(name, payload)`、`ctx.parallel_named`，载荷为 `Arc<dyn Any + Send + Sync>` | 同 `K-2` | 测试：命名事件的派发、监听器回退、重入语义与类型化事件一致 |
| `K-4` | 别名表保存 `ComponentId → ComponentId`，当前终点定义支持旧别名重新挂载；替换时释放旧定义。M10 增加 `Registry::forget`，在终点实例全部释放后显式退役该终点及相关别名（`ADR-0016`） | 定义闭包持有对应库的代码租约，替换和库关闭都需要明确的释放时机 | 测试：连续替换的旧闭包 `Weak` 释放；终点重挂；活跃实例保护及显式退役后的租约释放 |
| `K-5` | 调和循环的复位守卫 | 已在 `940650c` 完成 | 该提交附带的测试继续保留 |

命名键与类型化键共用 `DOC-01` 的全部条款。`K-2`、`K-3` 是在现有键表上的另一种入口，不复制余效应逻辑。

`Inject::with_named_options(name, required, predicate)` 接收命名依赖的必需标志与 `Option<DependencyPredicate>`。`DependencyPredicate` 为宿主拥有的 `Arc<dyn Fn(&AnyArc) -> bool + Send + Sync>`，由既有 `refresh` 在锁外调用。已有声明入口采用必需项和空谓词。可选项缺席的纪元槽为 `0`，存在时使用真实绑定编号并登记依赖关系；提供方等待实际使用方清理。版本不足按该项的必需标志决定 `Pending` 或可选缺席，具体语义见 `ADR-0016`。

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
| `#[interface(name = "kv", version = "1.0")]` | 接口 trait 与 impl | 类型化方法表、按方法名的通用入口、提供方蹦床、使用方代理、Interface 实现；代理在 flags 位 0 为 0 时通过 CBOR 通用调用（V-2） |
| `#[event(name = "message", version = "1.0")]` | 事件结构体（serde） | `Event` 实现 |
| `export_plugin!(name, version, components = [...])` | 组件类型列表 | `CORDIS_PLUGIN_ABI`、`cordis_plugin_v1`、静态描述符 |

宏生成的 `unsafe` 使用固定模板：调用 `cordis-abi` 公开的 unsafe 函数、按接口声明调用 C 函数指针、实现 C 蹦床、导出符号属性以及持有静态 ABI 表的 Send/Sync 实现。模板的 SAFETY 依据同时保存在宏源码与生成的文档注释中（规则 `B-2`、`ADR-0015`）。

### 3.4 `cordis-host`

| 模块 | 职责 |
|---|---|
| `library` | 复制到唯一路径（`SW-1`）、`libloading` 打开、握手、描述符校验 |
| `lease` | `LibraryId` 分配、租约计数、静止判定 |
| `adapt` | 实现 HostV1：provide、get、effect/defer、on、emit、export_state 经 UP/1 登记到核心；now、sleep 由宿主执行器完成；log 转发到核心 |
| 库替换资源 | 按核心的替换与回滚决策准备候选库、切换对象来源、释放旧库租约 |
| 对象表 | 维护 UP/1 编号与本地 AbiObject、回调、Future 的对应关系，移除时归还租约 |
| UP/1 桥 | 执行核心下发的 apply、callback、invoke、export、thaw、release、shutdown，代表分组中的原生插件与核心通信 |

系统的内核位于核心进程，是实例、依赖、纪元、加载、卸载、替换和回滚的唯一决策者，组内原生插件的依赖同样由核心解析并下发。cordis-host 使用普通数据结构和 RAII 管理库、租约、编号及原生引用，执行核心指令。标准输入关闭时宿主退出。

取得服务、提供服务和登记效应都经核心记录。get 的应答指向同组对象时，宿主交出本地 AbiObject，后续方法通过 ABI 类型化方法表直接调用；跨进程对象由 UP/1 代理实现。M11 验证宿主依赖树与内核 crate 分离，并验证同组 A 依赖 B 时，替换 B 由核心安排 A 先清理、B 后卸载，随后 A 的直连引用指向新 B。

### 3.5 `cordis-runtime`

- 可执行文件 cordis 提供 run、status、service install、version；其余命令原样转交同目录或 PATH 中的 cordis-dev（ADR-0024）。
- tokio 多线程运行时实现 `Spawn + Clock`。
- 配置：`cordis.toml`（`DOC-11` §10）。配置解析失败时启动失败；运行中配置文件变为无效时保持当前状态并记录错误。
- 管理接口 cordis.manage@1 按配置中的 grants 检查每个方法，串行执行变更并保存 state/instances.toml 叠加层。watch 插件经此接口触发安装、替换和配置重读。
- 状态目录布局：`state/<实例名>.state`、`state/consumed/`、`state/data/<插件名>/`（`cordis.data-dir@1`）、`state/status.json`。
- `status.json`：每个库的插件名、版本、副本序号和路径、是否验证、租约数、静止状态；每个实例的状态、纪元、最近一次替换报告。
- 信号：`SIGTERM`/`SIGINT` 触发 `DOC-11` §9 的有序退出。

cordis-dev 提供 new、new-plugin、verify、pack、idl gen，随对应里程碑接入。原生描述符由 cordis-host --describe 读取；验证由独立 cordis run 部署执行；模板与 M15 skill 共用。开发工具使用协议数据和子进程，产品依赖与内核、ABI、SDK 分离。

### 3.6 `cordis-contract`

- 复用 SDK 的内置接口 `cordis.contract@1`：`report(name, passed, message)`、`finish()`、`stage()`、`request_state_roundtrip()`。
- 提供断言辅助，套件作者用它编写套件插件。
- 套件经 UP/1 报告断言与阶段结果；核心按协议数据完成验证判定。

### 3.7 `cordis-up` 与 `cordis-supervisor`

cordis-up 保存双方共用的消息类型、帧大小约束、CBOR 深度与集合长度约束，并提供有界双向 RPC 连接、截止时间和载荷引用所有权。cordis-supervisor 负责进程组、握手、对象编号与服务路由、唯一内核中的生命周期控制、退出回收及有界重启。两者遵守 CP-1–CP-10，协议字段在 M10 的 DOC-16 中确定。

## 4 SDK 接口

```rust
use cordis_sdk::prelude::*;

#[cordis_sdk::interface(name = "kv", version = "1.0")]
pub trait Kv: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;
    fn keys(&self) -> Result<Vec<String>>;
    async fn flush(&self) -> Result<()>;
}
```

`#[interface]` 同时用于 trait 和实现该 trait 的 impl；异步方法转换为具有 Send 约束的 BoxFuture，代理以 `Service<dyn Kv>` 交给使用方。返回字符串使用 String，返回字节使用 Vec<u8>；其余 DTO 经 CBOR 编码。`#[event(name, version, kind = "sync" | "async")]` 声明事件派发族。

| 对象 | 接口及行为 |
|---|---|
| `Component` | NAME、Config、inject/provides/listens/state、resolve、异步 apply；apply 返回 `Result<()>` |
| `Context` 服务 | `provide::<dyn I>(Arc<dyn I>) -> Result<()>`；`get::<dyn I>() -> Result<Service<dyn I>>`；`data_dir() -> Result<PathBuf>`，使用方声明 DataDir 依赖 |
| `Context` 派生 | `derive(&ContextOptions) -> Result<Context>`；`isolate::<dyn I>(Option<&str>)` 建立命名或独立域；`intercept::<dyn I, M>(&M)` 在新视图中覆盖接口元数据 |
| `Context` 组合 | `plugin::<C>(&C::Config)`、`plugin_named(component, &config)` 异步返回子实例编号，组件来自调用方插件的描述符 |
| `Scoped<I>` | `Scoped::new(Arc<I>, factory)` 保存基对象与元数据视图工厂；`provide_scoped::<dyn I>(Scoped<I>)` 声明按上下文提供视图，组内继续使用类型化调用 |
| `Context` 效应 | 同步 effect 与异步 effect_async 返回 `Result<Effect>`；Effect 的 settled/dispose 都异步返回 `Result<()>`；EffectScope 支持 defer/defer_async |
| `Context` 任务 | spawn、spawn_thread 返回 `Result<()>`；`is_current() -> Result<bool>`；sleep 异步返回 `Result<()>` |
| `Context` 状态 | `inherited_state() -> Result<Option<StateBlob>>`；export_state 返回 Frozen；export_state_with 接收与写路径共享的 Frozen |
| `Context` 事件 | on/emit 对应 SyncEvent；on_async/emit_async 对应 AsyncEvent；全部通过 Result 表达失败 |
| `export_plugin!` | 导出入口、ABI 主版本和描述符；可选 rlib_feature 供回环测试链接，独立 cdylib 构建使用标准 C 符号 |

完整可编译实现位于 `plugins/kv-api/src/lib.rs`、`plugins/kv-mem/src/lib.rs` 和 `plugins/counter/src/lib.rs`。kv-mem 在状态锁内检查 Frozen 后写入；导出时先设置冻结标志再取得状态锁。counter 成功写入 KV 后更新本地计数，加载时从继承状态与 KV 当前值中取较大者。`crates/cordis-sdk/tests/loopback.rs` 通过实际入口执行基础组件与宿主操作，effect_handles 检验外部效应句柄释放；28 槽 HostV1 的派生、子组件和 Scoped 路径由 `crates/cordis-host/tests/native.rs` 的同组、跨组真实宿主场景覆盖。

## 5 运行时结构

### 5.1 生命周期与资源的归属

| 所在进程 | 核心对象 | 责任 |
|---|---|---|
| cordis | 根上下文、实例纤程、服务绑定、依赖纪元、效应树 | 决定加载和清理顺序、替换、回滚与崩溃回收 |
| cordis-host | 库句柄、ABI 租约、对象与回调编号、本地原生引用 | 执行核心请求，持有资源并按请求释放；同组方法直接调用 |

### 5.2 一次服务调用的路径

同组调用 `kv.put(...)` 时，插件侧代理通过类型化方法表进入提供方蹦床。跨组调用时，宿主提供 flags 位 0 为 0 的代理对象，SDK 将参数编码为 CBOR 数组，经 invoke 入口交给宿主，再由核心通过 UP/1 路由到提供方。服务对象在协议中使用拥有明确生命周期的编号。

### 5.3 内置接口

| 接口 | 提供方 | 内容 |
|---|---|---|
| `cordis.data-dir@1` | 运行时 | 返回实例所属插件的数据目录（`PR-7`） |
| `cordis.contract@1` | 验证核心，经 UP/1 | 契约结果、完成通知、阶段查询与状态往返请求（ADR-0017） |
| `cordis.runtime@1` | 运行时 | 只读查询：已装载库、实例状态（供管理类插件使用） |
| `cordis.manage@1` | 运行时，经 UP/1 | install、add、remove、replace、rollback、configure、reload、shutdown；授权来自实例条目的 grants（ADR-0024） |

运行时通过 UP/1 通知发布 `cordis.log@1`，载荷含 level、message、instance。跨进程监听器异步接收，服务调用承担需要返回值的交互（DOC-15 §6.3）。

## 6 架构规则

| 编号 | 规则 |
|---|---|
| `B-1` | 内核 crate 继续遵守第一阶段的 `A-1`–`A-12`，不使用 `unsafe`，不依赖 tokio |
| `B-2` | 核心进程的全部 crate 使用安全 Rust，依赖树与 cordis-abi、cordis-sdk 分离。unsafe 位于 cordis-abi、cordis-host 与宏生成的固定代码形态；每个 unsafe 块、函数、impl、extern 块和属性前一行有 SAFETY 注释说明 DOC-12 契约，宏模板同时生成相应文档。源码审计使用 syn 解析。cordis-abi 与 cordis-host 开启 deny(unsafe_op_in_unsafe_fn) |
| `B-3` | 同组原生边界使用 DOC-12 类型，跨进程使用 UP/1；SDK 的公开类型与内核类型分离（Q3） |
| `B-4` | 每个跨边界函数在被调用侧捕获 panic（`DOC-12` §6） |
| `B-5` | 宿主侧每个收到的对象都包装为持有租约的值（`DOC-12` §11） |
| `B-6` | 核心管理连接、实例、服务与插件登记的效应；宿主用 RAII 持有本地库、租约和对象表，按核心指令释放。watch 的监视器作为插件效应登记 |
| `B-7` | `SW-1`–`SW-4` 保持实例、纪元和绑定原状；`SW-5` 仅进入可逆冻结，导出失败时解冻；`SW-6` 开始生命周期替换（`DOC-11` §6） |
| `B-8` | 内核锁、路由锁与宿主资源锁在远端等待、插件调用和用户资源析构前释放 |
| `B-9` | 原生代码在宿主执行器的工作线程或调用方线程执行；UP/1 读写与 EOF 监测独立调度，插件系统线程经显式资源接口登记 |
| `B-10` | `docs/abi/v1-layout.txt` 在 M11 冻结前的修改记入 PROGRESS，冻结后随 ADR 修改 |

## 7 依赖白名单

内核 crate 的白名单不变（`DOC-02` §9）。其余 crate 的运行时依赖取自下表，新增需要写 ADR。

| crate | 用途 | 使用者 |
|---|---|---|
| `serde` | 配置、DTO、状态块的序列化 | `cordis-abi`、`cordis-sdk`、`cordis-runtime`、`cordis-types`、`cordis-up`、插件 |
| `serde_json` | JSON 验证印记、状态与报告 | `cordis-host`、`cordis-runtime`、`cordis-dev` |
| `semver` | 插件版本解析与接口数值版本处理 | `cordis-host`、`cordis-supervisor`、`cordis-runtime` |
| `clap` | 命令行参数与子命令解析 | `cordis-runtime`、`cordis-host`、`cordis-dev` |
| `ciborium` | CBOR 编解码（`ADR-0010`） | `cordis-abi`、`cordis-sdk`、`cordis-host`、`cordis-up` |
| `libloading` | 打开动态库 | `cordis-host` |
| `tokio` | 执行器、计时、双向消息与子进程 I/O | `cordis-runtime`、`cordis-supervisor`、`cordis-host`、`cordis-up`、`cordis-process`、`cordis-dev` |
| `nix` | 安全的 POSIX 进程组信号接口（ADR-0021） | `cordis-process` |
| `rustix` | 安全的 waitid 状态观察与待回收子进程管理（ADR-0027） | `cordis-process` |
| `libfuzzer-sys` | CP-9 输入模糊测试（ADR-0021） | 独立 fuzz 工作区的开发构建 |
| `notify` | 目录监视 | `plugins/watch` |
| `uuid` | 随机批次请求标识（ADR-0031） | `plugins/watch` |
| `chrono` | 终端日志时间格式（ADR-0032） | `plugins/console-log` |
| `toml` | 解析 `cordis.toml` 与生成项目配置 | `cordis-runtime`、`cordis-dev` |
| `syn`、`quote`、`proc-macro2` | 过程宏 | `cordis-sdk-macros` |
| `parking_lot`、`futures-util`、`thiserror` | 与内核相同 | 各 crate |

开发依赖不受此表限制，但需要在 `PROGRESS.md` 记录新增项。
