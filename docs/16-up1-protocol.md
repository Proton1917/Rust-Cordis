# 16 UP/1 通用插件通信协议

| 项 | 内容 |
|---|---|
| 文档编号 | `DOC-16` |
| 状态 | UP/1 候选规范（2026-11-12） |
| 依据 | `DOC-15`；`ADR-0018`–`ADR-0025`、`ADR-0028` |
| 实现 | `cordis-up` 的消息编解码与双向连接；`cordis-supervisor` 的进程监管和核心适配 |
| 读者 | 核心、原生宿主、通用插件 SDK 与转发插件的实现者 |

## 1 执行主体

核心进程中的内核统一决定实例、依赖、纪元、加载、清理、替换与回滚。每个插件进程是核心的一条连接；原生分组中的所有插件共用该分组连接。组件的服务、效应、监听器和状态导出器都登记在核心中，同组依赖也由核心解析。

`cordis-host` 依据本协议执行函数，使用普通数据结构及 RAII 管理动态库、租约、对象、回调、Future 和上下文数据。宿主依赖 `cordis-abi`、SDK 与 `cordis-up`。核心侧 `cordis-up`、监管和运行时使用安全 Rust，接口数据由本协议定义。

同组原生服务的快速路径发生在核心完成 `get` 解析之后：核心返回本地对象位置，宿主克隆该对象，后续方法通过类型化函数表直接调用。加载与清理次序继续由核心决定。

## 2 传输与编码

进程连接以标准输入输出传送二进制帧。核心写入插件标准输入，读取插件标准输出。每帧为四字节大端无符号长度 `L`，后接恰好 `L` 字节的 CBOR 消息体。帧头长度上限在分配消息缓冲前执行，解码器消费完整消息体；截断与尾随数据报告协议错误。

标准错误输出承载文本诊断，由核心逐行转入日志。插件日志也可使用 §8 的 `log` 消息。

CBOR 采用 RFC 8949 的类型。文本为 UTF-8；数组保持顺序；映射的键顺序具有相同语义。协议字段使用下表中的类型。

| 类型 | 编码 |
|---|---|
| `u64`、`u32`、`u8` | 对应范围的无符号整数 |
| `bool` | CBOR 布尔 |
| `String` | UTF-8 文本 |
| `Vec<T>` | CBOR 数组 |
| `Option<T>` | `T` 的值或 `null` |
| `Value` | 一项 CBOR 值，遵守 §3 的预算 |
| 单位结果 | `null` |
| 接口资源值 | 应用标签 `60000` 包裹非零 `u64` 编号，例如 `60000(7)` |

接口调用的 `args` 是按参数声明顺序排列的数组，返回值是一项 `Value`。资源编号参与路由；普通应用整数保持原值。诊断详情和状态数据承载可持久化的普通值，资源标签出现在其中时报告 Schema 错误。新上下文通过 `get` 重建运行时对象引用。

接口值中的 `Option` 按 `DOC-12` 编码：`None` 为 `null`；`Some` 通常采用内部值，内部值为 `null` 或标签 `60001` 时再包裹一层 `60001`。解码时，`null` 对应 `None`，`60001(payload)` 对应 `Some(payload)`，其余值对应 `Some(value)`；递归应用该规则保留嵌套层次。协议字段自身的 `Option<State>`、`Option<u64>` 等采用值或 `null` 的字段表示。

`cordis-up::encode` 和 `decode` 处理 CBOR 消息体。双向连接负责四字节帧头、异步读写、队列和请求表。`Connection::attach` 接受任意 `AsyncRead` 与 `AsyncWrite`，使连接语义独立于传输实现。

## 3 解析与连接预算

| 参数 | 默认值 | 含义 |
|---|---:|---|
| `max_frame` | 16777216 字节 | 消息体上限；双向连接采用至少 64 字节以容纳控制消息，独立 codec 接口保留自身输入范围 |
| `max_depth` | 64 | CBOR 容器与标签嵌套深度，包含消息包络；可配置范围为 4–128 |
| `max_collection` | 65536 | 参数、成功结果或诊断值图中单个数组的元素数、单个映射的键值对数 |
| `max_values` | 262144 | 参数、成功结果或诊断值图的总节点预算；映射的键与值分别计数，标签及其载荷分别计数 |
| `max_inflight` | 64 | 每个方向的在途请求上限 |
| `queue_capacity` | 64 | 每条连接的发送队列容量 |
| `request_timeout` | 30 秒 | 通用请求的默认截止时间；核心按操作类别选择有效时限 |
| `consecutive_timeouts` | 3 | 连续超时触发连接终止的阈值 |
| `max_exports` | 65536 | 核心向单条连接导出的活跃对象别名上限 |
| `max_registrations_per_context` | 65536 | 单个上下文的效应、清理、监听、服务及相关控制记录额度 |
| `max_retained_metadata_bytes` | 67108864 字节 | 核心持续保留的描述符、接口和方法等元数据额度 |

编解码使用 `ciborium`。解码期间，Serde visitor 与 `DeserializeSeed` 在集合增长和进入子值时消耗预算；明确长度与不定长集合执行相同上限。每次同步解码的配置由作用域守卫传入，返回与错误路径都恢复先前的线程局部配置。整个值图的节点预算限制大量短标量造成的内存放大。

收到的帧长度、CBOR、包络或资源表违反协议时，该连接失败。核心回收该连接的对象和登记内容，并按监管策略处置对应进程。本地输出超出连接编码容量时返回 `code = 8`，释放尚处于待交付状态的 `Outgoing`，目标连接保持运行。响应结果超限时交回可容纳的小型错误响应。连接的背压和截止时间独立执行，核心持有内核锁、路由锁期间只操作自身数据结构。

登记占用的额度由实际效应及其外部控制记录共同持有，相关持有结束后归还。额度耗尽返回 `code = 8`。上下文或连接退役时回收相应记录和计数。

## 4 消息包络

消息编码为以类别名为键的单键映射。下列代码块使用 CBOR 诊断表示；字符串键和大小写属于协议。

```text
{"request": {"id": 1, "method": "hello", "params": {"protocol": 1, "process": 4, "group": "storage"}}}
{"response": {"id": 1, "result": {"ok": {"protocol": 1, "plugins": []}}}}
{"response": {"id": 2, "result": {"error": {"code": 3, "message": "unknown method", "detail": null}}}}
{"notification": {"method": "release", "params": {"objects": [7]}}}
```

| 类别 | 字段 | 规则 |
|---|---|---|
| `request` | `id: u64`、`method: String`、`params: Value` | `id` 非零，`method` 非空，接收方交回同编号响应 |
| `response` | `id: u64`、`result` | `result` 是单键映射 `ok: Value` 或 `error: WireError` |
| `notification` | `method: String`、`params: Value` | 接收方执行通知语义 |

`WireError` 的字段为 `code: u32`、`message: String`、`detail: Value`。`detail` 是普通诊断数据。

| code | 含义 |
|---:|---|
| 1 | 一般执行失败 |
| 2 | 参数或数据格式无效 |
| 3 | 协议、接口版本、方法或能力缺失 |
| 4 | 上下文、对象或控制句柄失效，或已提交视图中的可选依赖缺席 |
| 5 | 插件执行发生 panic |
| 6 | 截止时间到达 |
| 7 | 调用被取消 |
| 8 | 连接或上下文额度耗尽 |
| 9 | 管理方法或来源路径的权限不足 |
| 1000 及以上 | 接口定义的业务错误 |

## 5 编号与握手

| 编号 | 分配方 | 作用域与有效期 |
|---|---|---|
| 请求 `id` | 每个请求发送端 | 两个方向分别单调分配；响应查找接收方自己发出的请求表 |
| `process` | 核心 | 当前连接世代；进程重启或重新打开转发通道时分配新值 |
| 实例身份 | 核心内核 | 替换期间保留实例身份，由配置实例名定位 |
| `context` | 核心 | `apply` 分配基础视图，`context_derive` 分配派生视图；同一加载的全部视图共享纪元与清理过程 |
| `scope` | 核心 | 一次效应体执行期间有效；清理和事件回调使用 `0` 表示该字段为空 |
| `effect` | 核心 | 外部效应控制句柄；每条连接单调分配 |
| 对象、回调、导出器 `object` | 出口表所在发送端 | 每条连接独立的单调出口编号，持有与释放规则见 §6 |
| 原生 `library` | 原生宿主 | 当前宿主进程中的库编号，由核心选择执行目标 |

编号表示表项。连接世代与编号共同确定资源身份。既有代理继续指向创建它的连接，连接退役后访问返回 `code = 4`。

对象出口编号从 `1` 开始按连接单调分配，表项持有实际对象引用。原生对象的地址保存在宿主表项中，消息以表项编号标识该对象。核心分配的 `context`、`scope`、`effect` 在连接存续期间保持唯一；`context = 0` 为日志诊断预留。

核心首先发送 `hello` 请求。`Hello` 字段为 `protocol: u32`、`process: u64`、`group: String`。UP/1 的 `protocol` 为 `1`。成功响应直接包含 `Describe`，完成一轮握手。

| 描述类型 | 字段 |
|---|---|
| `Describe` | `protocol: u32`、`plugins: Vec<Plugin>` |
| `Plugin` | `name`、`version`、`sdk_version`、`build_info: String`；`format: u8`；`library: Option<u64>`；`components: Vec<Component>` |
| `Component` | `name: String`；`inject: Vec<Dependency>`；`provides`、`listens: Vec<String>`；`state_schema: String`；`state_version_min`、`state_version_max: u32` |
| `Dependency` | `interface: String`、`required: bool` |

`format = 1` 表示原生库，`2` 表示通用插口插件，`3` 为 WebAssembly 预留。原生描述携带 `library`，进程插件对应字段为 `null`。插件名、版本、接口标识与组件声明由核心按 `DOC-11` 校验。接口标识为 `<名称>@<主版本>.<次版本>`，服务键取 `<名称>@<主版本>`；最低次版本和可选性由每个使用方的依赖声明决定。

## 6 资源载荷与路由

带资源所有权的协议 payload 使用根字段 `refs: Vec<u64>`。每个编号非零且在该数组中唯一。协议根以外的应用数据放在 `args`、`value`、`config`、`payload` 等字段中；应用 DTO 的同名属性保持普通数据。

发送方为本次转交建立出口记录和相应持有权，把编号列入 `refs`。载荷中的资源值使用 `60000(id)`；协议明确指定的 `object`、`body`、`callback`、`exporter` 字段使用对应的整数编号。一次载荷内对同一资源的多次使用可以引用同一编号。

接收方收到请求、通知或成功响应时，由拥有型 `Payload` 持有 `refs`。`Payload.import(id)` 创建独立的本地引用份额。载荷结束时释放其自身份额；最后一个本地份额释放时，向原出口端发送：

```text
{"notification": {"method": "release", "params": {"objects": [7, 8]}}}
```

`release.objects` 指向接收方自己的出口表。已分配范围内的缺席编号按幂等释放处理；`get`、`invoke` 等访问缺席表项返回 `code = 4`。连接断开时整表回收，随后建立的连接使用新的编号空间。

路由转发为目标连接建立新的出口别名并改写相应资源标签。例如，来源连接的对象 `7` 经核心转发为目标连接的对象 `101`，核心记录 `101` 与来源资源的关系。目标的最后持有者释放 `101` 后，核心归还该别名持有的来源份额。不同连接的编号相同也分别解析。

`Outgoing` 先等待有界发送队列的容量，取得发送许可后，在同一段连续执行中提交出口引用并交出消息。排队期间取消会释放待发送载荷。响应在 oneshot 中同样使用拥有型 `Payload`；接收者取消、超时后的迟到响应及丢弃结果都会归还相应引用。

控制通知因队列满或 I/O 故障而入队失败时，使连接失败并触发整表回收。此规则覆盖析构路径发出的 `release` 和 `cancel`，使资源归还具有明确终点。连接的最小容量容纳正常控制消息，本地编码容量错误按上一节交回调用方。

## 7 上下文与生命周期

核心在依赖满足时分配 `context` 并发送 `apply`。该编号关联核心选定的实例、配置、继承状态和本次加载。宿主保存这些数据并执行组件函数，完成后返回响应；核心据此推进内核状态。

上下文有效期覆盖 `apply`、已登记效应和服务、监听器以及在途清理请求。核心以加载世代的共享持有关系管理这些操作，最后持有结束后退役上下文，并发送 `release_context` 通知。清理期间，`get` 继续读取该次加载的 `committed` 依赖；`config`、日志和计时继续有效。

`is_current` 请求由核心读取目标状态。目标失效后返回 `false`，而上下文仍可完成清理。上下文退役后，访问返回 `code = 4`。`release_context` 的参数为 `{context: u64}`，方向为核心到宿主。宿主据此收束该上下文的事件通知任务并移除本地数据；请求式 `apply` 与清理按核心生命周期完成。

`context_derive` 在已有视图上应用隔离域和拦截元数据。`isolate` 的每项为 `{interface: String, realm: Option<String>}`，名称相同的 `realm` 在一个核心中共享；`realm = null` 创建独立域。`intercept` 的每项为 `{interface: String, meta: Value}`，元数据为普通持久化值。两类接口名均接受 `<名称>@<主版本>` 和 `<名称>@<主版本>.<次版本>`，按主版本服务键解析。父层与当前层均为映射时，当前层逐字段覆盖同名键并保留其余父层键；其他值由当前层覆盖。

派生成功返回 `ContextDerived`，其中 `object` 是 `cordis.context-view@1.0` 资源，方法表为空、`local = null`，通过 §6 的持有和 `release` 规则管理。服务、效应、监听器和子组件保留自己登记时的视图；最后持有者释放后回收相应记录。加载纪元结束时统一撤回各派生视图，外部资源随后访问返回 `code = 4`。回调中的 `context` 使用实际登记视图编号。

`plugin` 从调用方当前加载所使用的插件描述符中选择组件，并使用同一原生库编号或通用插件连接执行。子组件通过调用方的上下文挂载，父效应负责完整清理。配置先经过该组件的 `resolve`，挂载成功返回核心分配的子实例编号。

父实例的 `dispose` 完成时，全部子实例均已结束，私有域的服务与效应均已撤回。顶层效应沿 S-FBR-12、S-FBR-13 按逆序启动并发清理；实例完成以生命周期等待结果为观察点，单个父级回调属于其中一项效应。

同组替换的依赖顺序由核心维持。A 注入同组 B 时，替换 B 先使核心中的 A 进入清理，A 的清理响应结束后 B 完成卸载；新 B 激活后，核心重新加载 A。A 再次 `get` 所得的本地位置指向新 B 的对象。宿主按照这些指令操作其资源表。

## 8 消息字段与结果

下表中的 `C→H` 表示核心到宿主，`H→C` 表示宿主到核心。`request` 的单位成功结果为 `null`。方法 payload 的 Rust 定义集中在 `cordis_up::payload`。

| 方法 | 方向、类别 | 参数字段 | 成功结果 |
|---|---|---|---|
| `hello` | C→H request | `protocol: u32, process: u64, group: String` | `Describe` |
| `resolve` | C→H request | `plugin, component: String, library: Option<u64>, config: Value` | `null` |
| `apply` | C→H request | `plugin, component: String, library: Option<u64>, context: u64, config: Value, inherited: Option<State>` | `null` |
| `provide` | H→C request | `context: u64, interface: String, object: u64, methods: Vec<String>, scoped: bool, refs: Vec<u64>` | `null` |
| `get` | H→C request | `context: u64, interface: String` | `GetResult` |
| `context_derive` | H→C request | `context: u64, isolate: Vec<Isolation>, intercept: Vec<Interception>` | `ContextDerived` |
| `plugin` | H→C request | `context: u64, component: String, config: Value` | `{instance: u64}` |
| `object_info` | 双向 request | `object: u64` | `ObjectInfo` |
| `invoke` | 双向 request | `object: u64, method: String, args: Vec<Value>, refs: Vec<u64>` | `InvokeResult` |
| `effect` | H→C request | `context: u64, label: String, body: u64, asynchronous: bool, refs: Vec<u64>` | `{effect: u64}` |
| `defer` | H→C request | `scope: u64, callback: u64, asynchronous: bool, refs: Vec<u64>` | `null` |
| `callback` | C→H request 或 notification | `object: u64, context: u64, scope: u64, payload: Value, refs: Vec<u64>` | request 返回 `null` |
| `effect_settled` | H→C request | `effect: u64` | `null` |
| `effect_dispose` | H→C request | `effect: u64` | `null` |
| `effect_release` | H→C notification | `effect: u64` | — |
| `on` | H→C request | `context: u64, event: String, callback: u64, asynchronous: bool, refs: Vec<u64>` | `null` |
| `emit` | H→C request | `context: u64, event: String, payload: Value, asynchronous: bool, refs: Vec<u64>` | `null` |
| `export_state` | H→C request | `context: u64, schema: String, version: u32, exporter: u64, refs: Vec<u64>` | `null` |
| `export` | C→H request | `object: u64, context: u64` | `ValueResult` |
| `thaw` | C→H request | `object: u64, context: u64` | `null` |
| `inherited_state` | H→C request | `context: u64` | `Option<State>` |
| `config` | H→C request | `context: u64` | `ValueResult` |
| `is_current` | H→C request | `context: u64` | `bool` |
| `log` | H→C request 或 notification | `context: u64, level: u32, message: String` | request 返回 `null` |
| `release` | 双向 notification | `objects: Vec<u64>` | — |
| `cancel` | 双向 notification | `id: u64` | — |
| `release_context` | C→H notification | `context: u64` | — |
| `shutdown` | C→H request | `null` | `null`；核心收到响应后关闭输入，宿主按 EOF 退出 |

`resolve` 在核心提交挂载或配置更新前调用。校验失败保留原配置、纪元和运行实例。`library` 为原生请求选定的库编号；核心验证候选或回滚时明确选择对应编号，宿主据此定位函数。

| 结果类型 | 字段 |
|---|---|
| `GetResult` | `object: u64, interface: String, methods: Vec<String>, local: Option<LocalObject>, refs: Vec<u64>` |
| `ObjectInfo` | `interface: String, methods: Vec<String>, local: Option<LocalObject>` |
| `LocalObject` | `process: u64, object: u64` |
| `InvokeResult` | `value: Value, refs: Vec<u64>` |
| `ValueResult` | `value: Value`，承载配置或导出的普通状态值 |
| `State` | `schema: String, version: u32, data: Value` |
| `ContextDerived` | `context: u64, object: u64, refs: Vec<u64>` |

`get` 按调用上下文的依赖声明和已提交视图选择服务，`refs` 包含返回对象持有权。`local.process` 为对象所在连接世代，`local.object` 为该宿主已有的出口记录。核心向同组原生调用方提供该位置，使其克隆本地 ABI 对象。`object_info` 查询已经持有的对象元数据，返回值保持借用性质。

`provide.scoped` 缺省为 `false`。提供方声明 `scoped = true` 且使用方视图带有非空元数据时，核心向该提供对象调用 `cordis.scope(meta)`，结果为相同接口的资源对象。核心按使用方视图和实际服务绑定缓存该结果，并返回对象所在宿主的本地位置；同组原生调用继续使用类型化方法表。元数据为 `null` 或空映射时直接返回原对象。视图回收时释放缓存中的对象持有权。

`invoke` 的原生接收端统一调用 `Object::invoke_async`，该入口同时执行接口定义中的同步与异步方法。

`effect` 由核心登记真实效应。在效应体内，核心分配 `scope` 并请求宿主执行 `body` 回调。同步形式等待回调完成后返回句柄；异步形式返回句柄，由 `effect_settled` 等待完成。回调对应的响应表示效应体完成。

`defer` 在当前作用域登记清理。核心恢复效应时以请求式 `callback` 执行清理，并等待其结果。`effect_dispose` 等待实际效应恢复；`effect_release` 只归还外部控制句柄。SDK 的最后一个 Effect 引用释放时发送该通知。实际效应继续由内核持有，直到显式 dispose 或实例清理。已分配范围内的重复句柄释放按幂等操作处理；已退役句柄的 settled/dispose 访问返回 `code = 4`。

事件的 `asynchronous` 保留同步和异步两种登记族，同名两族分别派发。两族远端监听器都收到 `callback` 通知。通知任务在宿主归属于对应上下文；请求式清理保持内核规定的顺序。核心内部需要返回值的 `bail`、`waterfall` 沿用内部事件接口。

`export_state` 登记可回退的导出器及其 `schema`、`version`。`export` 执行导出并返回 `ValueResult`，导出器同时冻结写入。核心对导出值执行 `persistent_value` 检查，再用已登记的模式名、版本和该值构造 `State`；替换中止时，核心调用 `thaw` 解冻。宿主将 ABI 导出的 CBOR 字节转换为 `ValueResult.value`，继承时按接口契约把 `State` 交给新实例。状态模式转换与接受行为由新组件代码决定。

`log.level` 为 `0 Error`、`1 Warn`、`2 Info`、`3 Debug`。`context = 0` 表示连接级诊断，其余日志归属对应配置实例名。宿主可依据 `apply` 保存本地配置和继承状态；时钟与计时也在宿主执行。后台任务和线程通过普通 `effect`/`defer` 登记取消、等待和 join，宿主执行资源操作，核心掌握其生命周期。

## 9 宿主模块准备与退役

| 方法 | 方向、类别 | 参数 | 结果 |
|---|---|---|---|
| `library_prepare` | C→H request | `path: String` | `{library: u64, plugin: Plugin}` |
| `library_activate` | C→H request | `library: u64` | `null` |
| `library_retire` | C→H request | `library: u64` | `{leases: u64, closed: bool, quiescence_ms: Option<u64>, retired_images: u64}` |

准备阶段装载指定副本并复制描述符，返回宿主分配的库编号。核心使用该编号执行候选的 `resolve`，完成兼容检查及状态准备后选择激活、实例替换或回滚。激活更新宿主的资源映射；实例与依赖状态由核心内核更新。

退役结果报告实际租约数、库句柄关闭状态、静止耗时和累积旧映像数。核心根据报告、截止时间及 `DOC-15` 的分组策略决定后续操作。原生组的有状态重启由核心导出状态、建立新连接、继承状态并终止旧进程。

网页宿主沿用这三个方法：`library` 表示页面中的 ES 模块句柄，`path` 为核心指定的版本 URL；`apply.library` 选择对应版本的组件。网页描述符采用 `format = 2`。退役解除宿主的业务对象引用，浏览器模块缓存随页面结束释放，`closed` 返回 `false`。页面组的存活与 WebSocket 连接一致，版本切换使用既有状态导出、清理、继承和失败恢复步骤（ADR-0041）。

替换报告的 `committed` 表示 SW-8 已确认候选且部署已切换，`succeeded` 表示后续清理也已完成。替换管理错误的 `detail` 由核心统一封装为 `{group, plugin: Option<String>, committed: bool, cause: Value}`；提交值由本次核心事务持有，插件诊断置于 `cause`。运行时据该次错误的提交事实发布部署叠加层，并把清理错误交回调用者。

## 10 截止时间、取消与退出

核心对请求按 apply、清理、导出和调用类别设置截止时间。到期时结束本地等待、归还该请求持有的载荷，并记录实例错误。连续超时、协议错误或连接退出触发该连接的整体回收。

`cancel.id` 标识通知发送方先前发出的请求，因此接收端查找自己的入站执行表。当前可取消的执行为 `invoke`；接收端结束该调用的 Future。`apply`、效应体及请求式清理保持惯性，发起端通过截止时间和核心进程处置策略处理故障。迟到响应进入拥有型载荷后按取消路径释放。

核心停止进程前以 `null` 参数发送 `shutdown`。宿主先返回成功响应，核心收到后关闭连接输入，宿主与 SDK 按 EOF 退出；超过约定时限由核心处理进程组。进程终止时，核心使其对象表、上下文、效应、服务和监听器整体退役，依赖方由内核重新计算目标状态；监管按配置的重启策略建立新连接。

## 11 转发通道

本节定义 `ADR-0023`、`ADR-0041` 的转发消息。cordis-channels 建立逻辑连接，cordis-web-deployment 通过唯一内核管理页面实例。

| 方法 | 方向、类别 | 参数字段 | 结果或含义 |
|---|---|---|---|
| `channel_open` | 转发方→核心 request | `channel: u64, metadata: Value` | `null`，建立独立逻辑连接 |
| `channel_frame` | 双向 notification | `channel: u64, frame: bytes` | `frame` 为一个完整的内层 UP/1 CBOR 消息体 |
| `channel_close` | 双向 notification | `channel: u64, reason: String` | 关闭该逻辑连接并回收其资源 |

转发方在当前物理连接中单调分配非零 `channel` 编号。`metadata` 为 `{context: u64, page: Value}`，包含发起转发的加载上下文和页面连接说明。核心先核对上下文的实际来源连接及当前加载状态，再按 `cordis.toml` 中该实例的 `forwarder` 授权接受 `channel_open`。后续帧绑定已登记的物理连接与通道编号。

原生 HTTP 插件取得 `cordis.forward@1.0` 服务后，由 Host 的安全对象包装把接口调用转换成上述消息。回程通过 `cordis.forward-receiver@1.0` 进入对应 WebSocket 的有界队列。Forward 是普通服务接口，HostV1 继续使用冻结的 ABI v1.0 布局。

每条通道独立执行 `hello`，具有独立的请求编号、对象表、上下文和解析预算，分别执行 `CP-2`、`CP-3`、`CP-8`。内层方法集合为普通插件协议方法；出现 `channel_open`、`channel_frame`、`channel_close` 时报告通道协议错误。转发方退出会使其全部通道按连接断开处理。

WebSocket 的一条二进制消息对应一个 CBOR 消息体。`http` 插件转交原字节，核心分别检查外层转发帧和内层消息的长度与解析预算。传输封装增加的字节计入外层帧上限，内层消息大小同时满足两层预算。

网页宿主 `cordis-web` 执行核心下发的加载与清理指令。核心为页面分配独立 realm，并管理页面中的子实例；页面关闭时回收这些实例及其登记内容。原生与进程插件的 `[web]` 部分共享其插件版本，替换和回滚按 `DOC-15` §11 统一推进。

## 12 后续接口的协议位置

`ADR-0024` 的管理接口通过普通 `get`/`invoke` 使用。授权来源为核心配置中的实例 `grants`，管理操作和叠加层持久化由 M12 实现。`cordis.runtime@1` 提供只读状态，`cordis.manage@1` 承载管理操作。

`cordis-dev`、`watch` 与窗口插件遵循各自里程碑：开发工具调用独立宿主获取原生描述符；watch 作为获授权插件发起管理请求；窗口作为进程插件接入。窗口和前端组件都使用本协议的连接、依赖和资源生命周期。

## 13 隔离、拦截与子组件协议

本节依 `ADR-0025` 定义 M11 的消息和句柄行为。M10 保留上下文多视图结构及视图缓存位置，扩展请求在功能启用前返回 `code = 3`。

| 方法或字段 | 方向 | 参数或类型 | 结果与语义 |
|---|---|---|---|
| `context_derive` | H→C request | `context: u64, isolate: Vec<{interface: String, realm: Option<String>}>, intercept: Vec<{interface: String, meta: Value}>` | `{context: u64}`；核心依次派生隔离与拦截视图 |
| `plugin` | H→C request | `context: u64, component: String, config: Value` | `{instance: u64}`；核心在该视图中挂载调用方插件自己的组件 |
| `provide.scoped` | H→C | `bool`，默认 `false` | 表明提供方支持按使用方元数据产生视图对象 |
| `cordis.scope` | 核心经 `invoke` 调用 | `args = [meta]` | 视图资源值及其 `refs`，采用普通 `InvokeResult` |

命名域在整个核心内共用命名空间，`realm = null` 创建新域。派生上下文与原上下文属于同一个实例，共享加载世代及退役时机；各视图分别保留隔离与拦截信息。`get`、`provide`、效应和事件接口都接受视图编号。

网页上下文的命名域归属其页面连接：同页相同域名共享身份，跨页分别解析。核心上下文中的页面命名空间随派生与子组件继承。后端服务可见性来自部署时的父视图；后端插件继续使用全局命名域（DOC-15 §11.2、ADR-0041）。

`plugin` 的组件来自调用方插件的已登记描述符。核心把子实例登记为调用方的效应，父实例停用时执行相应子实例清理；子实例的 `apply` 继续由核心向同一连接下发。跨插件的部署和管理使用核心配置或 `cordis.manage@1`。

核心按使用方视图取得接口的合并元数据。提供方声明 `scoped` 且该元数据非空时，核心经通用调用取得 `cordis.scope(meta)` 返回的视图对象，并按（使用方上下文，实际绑定）缓存；其他情况返回原服务对象。上下文或绑定退役时释放相应缓存和资源份额。

视图对象与原对象位于同一宿主，仍可使用 §8 的本地位置完成同组直接调用。提供方通过视图对象保存元数据，核心继续控制依赖关系与效应归属。部署层的 `isolate`、`intercept` 进入 M12，后续脚本语言和网页宿主沿用同一协议。
