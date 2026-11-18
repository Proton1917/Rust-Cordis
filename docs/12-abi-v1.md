# 原生插件二进制接口

ABI 规定两个独立编译的程序怎样交换数据和执行调用，包括机器入口、参数表示、结果、内存寿命及错误。业务接口描述一个服务的行为。两层契约的使用者不同：前者由宿主和 SDK 实现，后者由业务插件定义。下面的固定入口属于下一版设计；已部署的 ABI v1 见末节。

## 装载与绑定

插件以版本化引导入口声明 ABI 主版本和可用能力。宿主按该版本读取描述，取得组件、依赖、提供的接口及状态模式，再与部署要求匹配。候选在现有实例切换前完成装载与接口绑定。能力或版本冲突指明插件、方法、期望值和实际值，旧实例继续服务。

业务方法以接口名、主版本和稳定方法名识别。消费方根据共同声明生成类型化代理，提供方生成分派入口。绑定核对所需方法的参数、结果与同步或异步约定。方法顺序属于源码组织；兼容追加的方法有自己的身份。缓存的绑定随提供方加载代退役。改变业务语义通过新主版本与契约用例表达。

## 固定调用入口

| 动作 | 契约与责任 |
|---|---|
| 发起 | 调用者提交目标句柄、方法身份及编码后的参数。入口只在调用期间借用输入；需延后处理时取得明确的所有权。 |
| 完成 | 同步结果直接返回。异步请求通过调用编号交付一次终态；语言 Future、闭包、唤醒器及执行器由创建方持有。 |
| 释放 | 分配方回收缓冲区。可供另一插件使用的对象带有明确的释放操作，宿主按实例与加载代登记。 |
| 退役 | 取消与完成竞争时只接受一次终态，迟到的结果归还附带资源。调用和清理结束后再退役仍被引用的代码。 |

同宿主调用使用本进程内固定签名的入口，跨宿主调用经 UP/1。两种传输共享业务方法、值与错误的定义。目标设计将业务方法的类型变化限制在绑定检查和数据解码中。正式发布新 ABI 前，需要冻结机器签名、布局、所有权、线程和兼容期，并用不同编译器独立构建两端验证。

## 绑定、应答和卸载的完整过程

以 `store@1` 的写入方法为例。消费方声明所需的最低次版本、方法名与参数结果类型。绑定时，宿主为当前提供方和加载代建立本地句柄。Rust 侧仍使用生成的方法调用；SDK 在边界编码业务参数，提供方分派器按自身声明解码。新实现追加查询方法以后，旧消费方仍按原方法身份调用写入。需要新方法的消费方依较高的最低次版本绑定。

调用编号解决应答配对，业务操作标识解决提交状态查询，二者寿命不同。提供方将写入结果与业务操作标识一起持久提交后，返回一个小回执。消费方在应答传输中断后先按业务操作标识查询，已有提交直接返回原回执。对结果待确认且缺少幂等约定的写入，调用方保留操作标识并查询业务所有者，以查询结果决定后续动作。

异步调用从“已受理”进入“已完成、已取消或提供方退役”之一。发起方释放完成槽后，迟到应答只归还关联资源。正在执行的同步 FFI 调用必须等到返回才能释放其代码租约。设有截止时间的逻辑调用可以提前结束等待；加载器仍持有执行中的库，直至库代码真正退出。宿主发生进程级故障时，核心以连接寿命回收该组资源。这样既能让使用者及时结束等待，也能使动态库释放时机有实际依据。

## 大数据与业务提交

普通值承担小型参数和回执。文件、完整清单等随业务量增长的数据采用分页、字节流或版本化快照引用。写入者记录已提交的操作标识与快照，返回小型回执；应答传输失败后，调用者据此查询原操作结果。流在读完、取消或提供方退役时回收资源。

值超出传输预算和二进制调用签名冲突各有独立诊断。错误说明接口、方法、请求或返回阶段，以及触及的限额或版本；业务正文留在数据所有者处。[协议容量](16-up1-protocol.md) · [方法表的历史缺陷](adr/0056-typed-layout-digest.md)

## ABI v1 与兼容期

v0.2.5 使用宿主 ABI 1.1；主分支基线 85860d8 包含 ABI 1.2。v1 读取 `CORDIS_PLUGIN_ABI` 和 `cordis_plugin_v1`，使用 `HostV1`、`PluginV1`、`AbiObject`、`AbiBuf`、`AbiFuture` 及库租约。组内服务提供类型化方法表和按名称调用的入口。1.2 的新消费方比较布局标记与摘要，旧消费方沿其编译版本的逻辑执行。

兼容期内，旧库经匹配 v1 的宿主运行，新旧宿主经 UP/1 暴露公开契约。已安装应用可逐组升级；兼容支持期见 [ADR-0014](adr/0014-versioning.md)。[原始类型](../crates/cordis-abi/src/raw.rs) · [v1.0 布局基准](abi/v1-layout.txt) · [v1.1 布局基准](abi/v1.1-layout.txt) · [v1 全部槽位与字段](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/12-abi-v1.md)

## 附录　v1 机器边界速查

这些表供维护既有二进制程序时查询。v1 的拥有型缓冲在创建方释放；对象、回调、唤醒器与未终结的异步调用维持对应的代码租约。握手时使用描述符大小和版本决定可读取的范围。

| 类型 | 字段 | 用途 |
|---|---|---|
| `AbiStatus` | `u32`：`0` 成功，`1` 失败（错误写入调用方提供的 `*mut AbiError`），`2` 被调用侧发生 panic（错误同样写出） | 每个可失败函数的返回值 |
| `AbiStr` | `ptr: *const u8, len: u64` | 借用的字符串，仅在调用期间有效 |
| `AbiBytes` | `ptr: *const u8, len: u64` | 借用的字节串，仅在调用期间有效 |
| `AbiBuf` | `ptr: *mut u8, len: u64, cap: u64, owner: *mut c_void, free: Option<extern "C" fn(owner: *mut c_void, ptr: *mut u8, len: u64, cap: u64, err: *mut AbiError) -> AbiStatus>, library: LibraryId` | 拥有的字节缓冲、资源守卫及租约。`free` 为空时数据为静态数据或空值，调用方归还对应租约 |
| `AbiOption<T>` | `is_some: u8, value: MaybeUninit<T>` | 可选值 |
| `AbiError` | `code: u32, message: AbiBuf, detail: AbiBuf` | 错误。`message` 为 UTF-8 文本；`detail` 为可选的自描述编码数据（§7.3） |
| `AbiObject` | `data: *mut c_void, vtable: *const ObjectHeader, library: LibraryId` | 拥有的对象引用（§7）。`library` 为实现代码所在的库，由创建对象的一侧填写 |
| `AbiContext` | `context: u64, lease: AbiObject` | 派生上下文编号与一份拥有的视图租约 |
| `LibraryId` | `u64` | 宿主为每个装载的库分配的编号，0 保留给宿主自身 |
| `AbiUnit` | `reserved: u8`，值为 0 | 单位结果 |
| `AbiState` | `schema: AbiBuf, version: u32, reserved: u32, data: AbiBuf` | 拥有的状态块，reserved 为 0 |

### v1 的宿主操作

| 序号 | 函数 | 作用 | 对应内核接口 |
|---|---|---|---|
| 0 | `log(ctx, level: u32, message: AbiStr, err)` | 写日志；level：0 Error、1 Warn、2 Info、3 Debug。Drop/Wake 故障使用 0 | `ctx.logger()` |
| 1 | `now(ctx, out: *mut u64, err)` | 宿主时间，纳秒 | `Clock::now` |
| 2 | `sleep(ctx, nanos: u64, out: *mut AbiFuture, err)` | 计时等待 | `Clock::sleep` |
| 3 | `spawn(ctx, task: AbiFuture, err) -> AbiStatus` | 派发归属当前效应的任务（`PR-1`） | `effect_async` |
| 4 | `effect(ctx, label: AbiStr, body: AbiObject, out: *mut u64, err) -> AbiStatus` | 登记同步效应，`body` 为同步回调，收到效应作用域句柄 | `ctx.effect` |
| 5 | `effect_async(ctx, label, body, out, err) -> AbiStatus` | 登记异步效应 | `ctx.effect_async` |
| 6 | `defer(scope: u64, cleanup: AbiObject, err) -> AbiStatus` | 在效应作用域中登记同步清理 | `EffectScope::defer` |
| 7 | `defer_async(scope, cleanup, err) -> AbiStatus` | 登记异步清理 | `EffectScope::defer_async` |
| 8 | `is_current(ctx, out: *mut u8, err)` | 当前纪元是否仍有效 | `Context::is_current` |
| 9 | `provide(ctx, interface: AbiStr, value: AbiObject, scoped: u8, err) -> AbiStatus` | 提供接口实现并登记效应；scoped 为 1 时支持 cordis.scope，0 表示普通提供方 | `ctx.provide`（命名键，`K-2`） |
| 10 | `get(ctx, interface: AbiStr, out: *mut AbiObject, err) -> AbiStatus` | 取得已注入接口的实现 | `ctx.get`（命名键） |
| 11 | `on(ctx, event: AbiStr, listener: AbiObject, err) -> AbiStatus` | 登记事件监听器，登记为效应 | `ctx.on`（命名事件） |
| 12 | `emit(ctx, event: AbiStr, payload: AbiBytes, err) -> AbiStatus` | 同步派发事件，载荷为 CBOR | `ctx.emit` |
| 13 | `emit_async(ctx, event: AbiStr, payload: AbiBytes, out: *mut AbiFuture, err)` | 异步并行派发 | `ctx.parallel` |
| 14 | `export_state(ctx, schema: AbiStr, version: u32, exporter: AbiObject, err) -> AbiStatus` | 登记状态导出器（`DOC-11` §5），登记为效应 | 运行时 |
| 15 | `inherited_state(ctx, out: *mut AbiOption<AbiState>, err)` | 读取继承状态 | 运行时 |
| 16 | `config(ctx, out: *mut AbiBuf, err)` | 读取实例配置，CBOR | `Fiber::config` |
| 17 | `spawn_thread(ctx, name: AbiStr, body: AbiObject, err) -> AbiStatus` | 创建归属当前效应的系统线程（`PR-2`） | 运行时 |
| 18 | `lease_acquire(library: LibraryId, err)` | 增加租约（§11） | 装载器 |
| 19 | `lease_release(library: LibraryId, err)` | 减少租约 | 装载器 |
| 20 | `effect_settled(effect, out: *mut AbiFuture, err)` | 等待效应体完成 | `Effect::settled` |
| 21 | `effect_dispose(effect, out: *mut AbiFuture, err)` | 释放效应并等待清理 | `Effect::dispose` |
| 22 | `object_export(value: AbiObject, out: *mut u64, err)` | 消费对象并分配资源编号 | 宿主资源表 |
| 23 | `object_import(id: u64, out: *mut AbiObject, err)` | 克隆资源表中的对象 | 宿主资源表 |
| 24 | `object_release(id: u64, err)` | 移出并释放资源表引用 | 宿主资源表 |
| 25 | `effect_release(effect: u64, err)` | 释放 SDK 外部效应句柄记录；实际效应由 dispose 或所属实例停用恢复 | 核心效应句柄表 |
| 26 | `context_derive(ctx, options: AbiBytes, out: *mut AbiContext, err)` | options 为包含 isolate/intercept 数组的 CBOR；返回派生编号与视图租约 | `isolate_named`、`intercept_named` |
| 27 | `plugin(ctx, component: AbiStr, config: AbiBytes, out: *mut AbiFuture, err)` | 在给定视图中装入调用方插件自己的组件；Future 成功值为 u64 实例编号 | 组件装载 |

### v1 方法参数的表示

| Rust 类型（接口 crate 中） | 跨边界形式 |
|---|---|
| `bool`、`i8`–`i64`、`u8`–`u64`、`f32`、`f64` | 原样，`bool` 按 §2 |
| `&str` | `AbiStr` |
| `String` | `AbiBuf`（UTF-8） |
| `&[u8]` | `AbiBytes` |
| `Vec<u8>` | `AbiBuf` |
| `Service<dyn I>`（另一接口的对象） | `AbiObject` |
| `Option<T>`，`T` 为以上类型 | `AbiOption<T>` |
| 其他实现 `serde::Serialize + DeserializeOwned` 的类型 | `AbiBuf`，内容为 CBOR（RFC 8949）编码 |
| `Result<T>` | 方法返回 `AbiStatus`，成功值写入 out 指针，错误写入 `AbiError` |
| `()` | `AbiUnit` |
