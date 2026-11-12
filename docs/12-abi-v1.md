# 12 插件二进制接口 ABI v1

| 项 | 内容 |
|---|---|
| 文档编号 | `DOC-12` |
| 状态 | 草案。`M8` 结束时冻结为 ABI v1，此后布局不再改变（§12） |
| 读者 | 实现者；为其他语言编写插件绑定的人 |
| 输入 | `DOC-10` 公理 `Q3`、`Q4`、`Q8`；`DOC-11` 的插件模型；`ADR-0010`、`ADR-0012` |
| 输出 | 平台约定、入口与握手、基本类型布局、所有权规则、错误与 panic 规则、对象与虚表、异步约定、宿主虚表、插件描述符、租约、兼容规则 |
| 不包含 | 插件作者使用的 Rust 接口（`DOC-13` §4）；装载器内部实现 |

本文是规范性文档。`crates/cordis-abi` 是它的 Rust 实现，`docs/abi/v1-layout.txt` 是由测试生成的布局记录，三者必须一致（`DOC-14` 退出条件 `Y-7`）。

## 1 设计依据

| 要求 | 来源 | 本文的对应做法 |
|---|---|---|
| 插件可以用不同编译器版本构建 | `G7` | 只用 C 调用约定与 `#[repr(C)]` 布局；不传递任何 Rust 标准库类型 |
| 布局由本项目规定，不随第三方 crate 版本变化 | `ADR-0010` | 全部类型在本文中定义 |
| 不出现跨分配器释放 | `DOC-10` §8 第 1 项 | 拥有所有权的值携带自己的释放函数（§5） |
| 不出现跨边界展开 | 同上 | 每个边界函数在被调用侧捕获 panic（§6） |
| 旧库代码可判定静止 | `Q4` | 每个对象携带租约（§11） |
| 追加能力不破坏旧插件 | `ADR-0014` | 虚表首部记录长度，追加只在末尾（§7.2） |

## 2 平台约定

| 项 | 约定 |
|---|---|
| 目标平台 | `aarch64-apple-darwin`、`x86_64-unknown-linux-gnu`。均为 64 位小端 |
| 调用约定 | `extern "C"` |
| 指针与长度 | 指针 8 字节；长度与计数一律用 `u64`，不用 `usize` |
| 布尔 | `u8`，0 为假，1 为真，其他值视为协议错误 |
| 字符串 | UTF-8，不以 0 结尾，以指针加长度表示 |
| 线程 | 宿主函数可以从任意线程调用；插件导出的函数必须能从任意线程调用（`Send + Sync` 语义） |

## 3 入口与握手

插件库导出两个符号：

| 符号 | 类型 | 内容 |
|---|---|---|
| `CORDIS_PLUGIN_ABI` | `static u32` | 插件编译时使用的 ABI 主版本，v1 为 `1` |
| `cordis_plugin_v1` | `extern "C" fn(host: *const HostV1, library: LibraryId, out: *mut *const PluginV1, err: *mut AbiError) -> AbiStatus` | 入口函数 |

握手顺序：

1. 装载器读取 `CORDIS_PLUGIN_ABI`。值不在核心支持的主版本集合内时，关闭库并报错，不调用任何插件代码。
2. 调用 `cordis_plugin_v1`，传入宿主虚表和装载器为该库分配的 `LibraryId`。插件保存二者（宿主虚表在核心进程生命期内有效），写出描述符指针。
3. 装载器校验描述符（§10）。校验失败时调用描述符的 `release` 并报错。
4. 描述符在装载器调用 `release` 之前保持有效。

入口函数被调用至多一次。

## 4 基本类型

全部类型为 `#[repr(C)]`。

| 类型 | 字段 | 用途 |
|---|---|---|
| `AbiStatus` | `u32`：`0` 成功，`1` 失败（错误写入调用方提供的 `*mut AbiError`），`2` 被调用侧发生 panic（错误同样写出） | 每个可失败函数的返回值 |
| `AbiStr` | `ptr: *const u8, len: u64` | 借用的字符串，仅在调用期间有效 |
| `AbiBytes` | `ptr: *const u8, len: u64` | 借用的字节串，仅在调用期间有效 |
| `AbiBuf` | `ptr: *mut u8, len: u64, cap: u64, free: Option<extern "C" fn(ptr: *mut u8, len: u64, cap: u64)>` | 拥有的字节缓冲。`free` 为空表示无需释放（空缓冲或静态数据） |
| `AbiOption<T>` | `is_some: u8, value: MaybeUninit<T>` | 可选值 |
| `AbiError` | `code: u32, message: AbiBuf, detail: AbiBuf` | 错误。`message` 为 UTF-8 文本；`detail` 为可选的自描述编码数据（§7.3） |
| `AbiObject` | `data: *mut c_void, vtable: *const ObjectHeader, library: LibraryId` | 拥有的对象引用（§7）。`library` 为实现代码所在的库，由创建对象的一侧填写 |
| `LibraryId` | `u64` | 宿主为每个装载的库分配的编号，0 保留给宿主自身 |

`AbiError.code` 取值：`1` 一般错误，`2` 无效参数，`3` 版本不足（调用超出提供方次版本的方法），`4` 对象已失效（实例已停用），`5` panic，`6` 超时，`7` 被取消。`1000` 以上留给接口自定义。

## 5 所有权与内存

1. 借用类型（`AbiStr`、`AbiBytes`、指针参数）只在本次调用期间有效。被调用侧需要保留时自行复制。
2. 拥有类型（`AbiBuf`、`AbiError`、`AbiObject`、`AbiFuture`、`AbiWaker`）的释放由分配它的一侧提供的函数完成：`AbiBuf.free`，或对象虚表中的 `drop`。接收方用完后调用该函数，不调用自己的分配器。
3. 通过 `out` 参数写出的拥有类型，所有权转给调用方。函数返回 `AbiStatus` 非 0 时，`out` 不被写入，错误的所有权转给调用方。
4. 一个拥有值恰好被释放一次。

## 6 错误与 panic

1. 每个跨边界函数的实现都在函数体外层捕获 panic，转换为 `AbiStatus = 2` 与 `code = 5` 的 `AbiError`。展开不得越过边界。
2. 插件必须以 `panic = "unwind"` 编译（`DOC-11` `PR-8`），描述符字段 `panic_unwind` 为 `1`。
3. 宿主收到插件 panic 时，按内核的规则处理：发生在 `apply` 中时实例进入 `Failed`；发生在服务方法中时调用方收到错误；发生在效应清理中时记录错误并继续其余清理。

## 7 对象与虚表

### 7.1 对象

接口实现、回调、异步任务都以 `AbiObject` 跨边界传递。`vtable` 指向一个以 `ObjectHeader` 开头的虚表。虚表是实现方库中的静态常量，任何一方都不写入它；租约归属由对象的 `library` 字段给出：

```c
struct ObjectHeader {
    uint64_t  size;          // 虚表总字节数，含首部
    uint32_t  kind;          // 1 接口实现，2 同步回调，3 异步回调，4 状态导出器
    uint32_t  reserved;      // 0
    AbiStr    interface;     // 接口标识 "kv@1.3"；回调为空串
    AbiObject (*clone)(void *data);
    void      (*drop)(void *data);
};
```

`clone` 产生一份新的拥有引用，`drop` 释放一份。对象内部的引用计数由实现方维护。

### 7.2 方法与追加

接口的方法指针紧随 `ObjectHeader`，按方法在接口 trait 中的声明顺序排列。每个方法的签名形如：

```c
AbiStatus method(void *data, <参数...>, <返回值 out 指针>, AbiError *err);
```

次版本追加的方法排在末尾。调用方在调用第 `i` 个方法前检查 `size >= sizeof(ObjectHeader) + (i + 1) * 8`，不满足时返回 `code = 3`，不读取越界内存。

### 7.3 参数与返回值的编码

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

选用 CBOR 的理由见 `ADR-0010`：它是公开标准，自描述，字段按名称编码，追加带默认值的字段不破坏旧读取方（`DOC-11` §3.2 第 2 条）。

## 8 异步

```c
struct AbiFuture {                 // 不使用 ObjectHeader，单独定义
    void *data;
    const struct FutureVTable *vtable;
    LibraryId library;
};
struct FutureVTable {
    uint64_t  size;
    // 返回 0 表示 Pending；1 表示 Ready，结果写入 out；2 表示 Ready 且失败，错误写入 err
    uint32_t (*poll)(void *data, const struct AbiWaker *waker, void *out, AbiError *err);
    void     (*drop)(void *data);
};
struct AbiWaker {
    void *data;
    const struct WakerVTable *vtable;
    LibraryId library;             // 由宿主创建的唤醒器为 0
};
struct WakerVTable {
    struct AbiWaker (*clone)(void *data);
    void (*wake)(void *data);          // 消耗该引用
    void (*wake_by_ref)(void *data);
    void (*drop)(void *data);
};
```

1. 异步方法与异步回调返回 `AbiFuture`。轮询方按 Rust `Future` 的规则轮询，返回 Ready 后不再轮询，随后调用 `drop`。
2. 未完成即 `drop` 表示取消。实现方保证取消后不再访问 `waker`。
3. `AbiWaker` 由轮询方提供。实现方保存唤醒器时必须 `clone`。
4. 轮询 `AbiFuture` 的始终是宿主的执行器。插件不运行自己的执行器来轮询宿主交给它的 Future（`DOC-11` `PR-3` 所述的自有运行时只用于插件内部的 tokio 接口）。

## 9 宿主虚表 `HostV1`

宿主虚表以 `size: u64` 开头，其后为函数指针，按下表顺序排列。`ctx` 为宿主分配的上下文句柄（`u64`），在对应实例的效应回调期间以及实例存活期间有效；实例停用后使用句柄的调用返回 `code = 4`。

| 序号 | 函数 | 作用 | 对应内核接口 |
|---|---|---|---|
| 0 | `log(ctx, level: u32, message: AbiStr)` | 写日志 | `ctx.logger()` |
| 1 | `now(ctx) -> u64` | 宿主时间，纳秒 | `Clock::now` |
| 2 | `sleep(ctx, nanos: u64) -> AbiFuture` | 计时等待 | `Clock::sleep` |
| 3 | `spawn(ctx, task: AbiFuture, err) -> AbiStatus` | 派发归属当前效应的任务（`PR-1`） | `effect_async` |
| 4 | `effect(ctx, label: AbiStr, body: AbiObject, out: *mut u64, err) -> AbiStatus` | 登记同步效应，`body` 为同步回调，收到效应作用域句柄 | `ctx.effect` |
| 5 | `effect_async(ctx, label, body, out, err) -> AbiStatus` | 登记异步效应 | `ctx.effect_async` |
| 6 | `defer(scope: u64, cleanup: AbiObject, err) -> AbiStatus` | 在效应作用域中登记同步清理 | `EffectScope::defer` |
| 7 | `defer_async(scope, cleanup, err) -> AbiStatus` | 登记异步清理 | `EffectScope::defer_async` |
| 8 | `is_current(ctx) -> u8` | 当前纪元是否仍有效 | `Context::is_current` |
| 9 | `provide(ctx, interface: AbiStr, value: AbiObject, err) -> AbiStatus` | 提供接口实现，登记为效应 | `ctx.provide`（命名键，`K-2`） |
| 10 | `get(ctx, interface: AbiStr, out: *mut AbiObject, err) -> AbiStatus` | 取得已注入接口的实现 | `ctx.get`（命名键） |
| 11 | `on(ctx, event: AbiStr, listener: AbiObject, err) -> AbiStatus` | 登记事件监听器，登记为效应 | `ctx.on`（命名事件） |
| 12 | `emit(ctx, event: AbiStr, payload: AbiBytes, err) -> AbiStatus` | 同步派发事件，载荷为 CBOR | `ctx.emit` |
| 13 | `emit_async(ctx, event, payload) -> AbiFuture` | 异步并行派发 | `ctx.parallel` |
| 14 | `export_state(ctx, schema: AbiStr, version: u32, exporter: AbiObject, err) -> AbiStatus` | 登记状态导出器（`DOC-11` §5），登记为效应 | 运行时 |
| 15 | `inherited_state(ctx, out_schema: *mut AbiBuf, out_version: *mut u32, out_data: *mut AbiBuf) -> u8` | 读取继承状态，没有时返回 0 | 运行时 |
| 16 | `config(ctx, out: *mut AbiBuf)` | 读取实例配置，CBOR | `Fiber::config` |
| 17 | `spawn_thread(ctx, name: AbiStr, body: AbiObject, err) -> AbiStatus` | 创建归属当前效应的系统线程（`PR-2`） | 运行时 |
| 18 | `lease_acquire(library: LibraryId)` | 增加租约（§11） | 装载器 |
| 19 | `lease_release(library: LibraryId)` | 减少租约 | 装载器 |

`M8` 可以在冻结前调整此表的签名与顺序，调整后同步更新本节和布局记录。冻结后只能在末尾追加，并提升 `HostV1` 的次版本（`size` 增大）。

## 10 插件描述符 `PluginV1`

```c
struct PluginV1 {
    uint64_t size;
    uint32_t abi_major;           // 1
    uint32_t abi_minor;           // 插件编译时使用的 cordis-abi 次版本
    uint8_t  panic_unwind;        // 必须为 1（PR-8）
    uint8_t  format;              // 1 原生库；其他值保留给后续装载器
    uint8_t  reserved[6];
    AbiStr   name;                // 插件名
    AbiStr   version;             // 插件版本，semver
    AbiStr   sdk_version;         // cordis-sdk 版本
    AbiStr   build_info;          // 编译器版本、目标三元组，供诊断
    const struct ComponentV1 *components;
    uint64_t component_count;
    void (*release)(const struct PluginV1 *self);
};
struct ComponentV1 {
    uint64_t size;
    AbiStr   name;
    const struct DependencyV1 *inject;   uint64_t inject_count;
    const AbiStr *provides;              uint64_t provides_count;   // 声明提供的接口，用于 SW-4
    const AbiStr *listens;               uint64_t listens_count;    // 声明监听的事件，供诊断
    AbiStr   state_schema;                // 空串表示不导出状态
    uint32_t state_version_min, state_version_max;   // 接受继承的版本范围
    // config 为 CBOR；返回的 AbiFuture 完成即 apply 完成
    AbiStatus (*apply)(uint64_t ctx, struct AbiBytes config, struct AbiFuture *out, AbiError *err);
    // 配置校验，失败时 err.detail 为带路径的问题列表（CBOR）
    AbiStatus (*resolve)(struct AbiBytes config, AbiError *err);
};
struct DependencyV1 {
    AbiStr  interface;            // "kv@1.2"
    uint8_t required;             // 1 必需；0 可选
    uint8_t reserved[7];
};
```

装载器的校验：

1. `abi_major` 受支持，`size` 不小于 v1.0 的结构大小。
2. `panic_unwind == 1`。配置项 `allow_panic_abort = true` 时降级为警告。
3. `format == 1`。
4. `name` 与 `version` 非空且合法；组件名在库内唯一；接口标识符合 `DOC-11` §3.1。

## 11 租约

1. 装载器为每个装载的库分配 `LibraryId` 和一个原子计数。
2. 任何持有某库代码指针或数据指针的值（`AbiObject`、`AbiFuture`、`AbiWaker`、已登记的回调与清理函数），在存在期间持有该库的一份租约。
3. 宿主从插件接收此类值时，在宿主侧的包装中获取一份租约，包装被释放时归还。
4. 插件把从宿主取得的另一插件的对象再 `clone` 时，SDK 调用 `lease_acquire` 获取新租约；`drop` 时调用 `lease_release`。
5. 库的组件全部停用且计数为 0 时，该库的代码进入静止状态。之后是否关闭库由 `ADR-0012` 规定。

## 12 兼容规则与变更流程

| 变更 | 允许的时机 | 要求 |
|---|---|---|
| 修改任何已有类型的布局或函数签名 | 仅 `M8` 冻结前 | 冻结后需要发布 ABI v2 |
| 在 `HostV1`、`PluginV1`、`ComponentV1` 末尾追加字段或函数 | 冻结后 | ABI v1 次版本加一；读取方以 `size` 判断字段是否存在；写 ADR |
| 新增 `AbiError.code` 取值 | 冻结后 | 旧插件把未知值视为 `1` |
| 新的 ABI 主版本 | 核心年度升级 | 核心对旧主版本继续提供适配，支持期见 `ADR-0014` |

冻结后，`docs/abi/v1-layout.txt` 的任何改动都必须伴随一份 ADR。
