# UP/1 进程通信规约

UP/1 连接核心与一个原生宿主、脚本进程或网页转发通道。同宿主原生服务由宿主在本地执行。消息报告服务的身份、调用来源和资源引用；业务参数与状态内容由插件解释。

## 传输与错误

进程从标准输入读取消息并向标准输出写回，标准错误承载诊断。一帧由四字节大端长度和相应数量的 CBOR 字节组成。端点先确定消息边界，再解码包络、参数与资源编号。调用有明确的截止时间。协议格式错误关闭相应连接；业务侧本地编码超限应在该次操作返回具有位置和预算的错误。控制消息与大块业务内容分别处理。

## 容量与大结果

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

值图计数包括映射的键和值。字节数小于帧上限的内容，也可能先触及集合或节点上限。大清单写入返回操作编号和快照版本，完整内容用资源流或分页读取。调用者查询持久回执来区分写入失败与返回值传输失败。资源流按使用者的读取进度传送，取消和提供方退役都会释放相关引用。

## 消息、身份与资源

`hello` 建立连接与版本，`apply` 将组件激活交给执行端。`provide` 与 `get` 使内核解析绑定，`invoke` 向提供方送达方法、参数与真实调用方信息。调用编号配对请求和结果；资源引用由连接登记，普通整数依其业务类型解释。携带对象的失败调用和被丢弃的结果要归还各自持有的引用。清理先尊重依赖顺序，再撤回加载代。

`channel_open`、`channel_frame` 和 `channel_close` 对应转发连接的建立、帧传递与关闭。HTTP 插件解释 Cookie 和 WebSocket；核心确认通道与页面身份。进程退出时撤回连接的服务和效应，之后按配置恢复。

支持调用预算的端点在成功的 `hello` 结果中声明 `invoke_budget_v1: true`。握手发起方收到该能力后，可在 `invoke` 参数加入 `budget_ms: u64`，表示发送方当前剩余的毫秒数。对等端首次收到该字段后也可发送该字段。各端按本地单调时钟计算截止时间，核心为每次逻辑调用维护最终截止，转发与嵌套调用取父调用剩余预算与本地时限中的较小值。连接到原有端点时，核心继续使用发送方截止时间及 `cancel` 通知。插件加载和清理按其生命周期时限执行。原生同步调用的租约随实际执行及析构结束归还。

## 线上字段参考

以下字段表描述已有 UP/1 消息，提供方和消费方读取相应的类型及编号。新增原生 ABI 入口复用这些业务语义；目标设计对既有 UP/1 的字段调整须独立协商协议版本。

### 4 消息包络

| 类别 | 字段 | 规则 |
|---|---|---|
| `request` | `id: u64`、`method: String`、`params: Value` | `id` 非零，`method` 非空，接收方交回同编号响应 |
| `response` | `id: u64`、`result` | `result` 是单键映射 `ok: Value` 或 `error: WireError` |
| `notification` | `method: String`、`params: Value` | 接收方执行通知语义 |

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

### 5 编号与握手

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

| 描述类型 | 字段 |
|---|---|
| `Describe` | `protocol: u32`、`plugins: Vec<Plugin>` |
| `Plugin` | `name`、`version`、`sdk_version`、`build_info: String`；`format: u8`；`library: Option<u64>`；`components: Vec<Component>` |
| `Component` | `name: String`；`inject: Vec<Dependency>`；`provides`、`listens: Vec<String>`；`state_schema: String`；`state_version_min`、`state_version_max: u32` |
| `Dependency` | `interface: String`、`required: bool` |

### 8 消息字段与结果

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

| 结果类型 | 字段 |
|---|---|
| `GetResult` | `object: u64, interface: String, methods: Vec<String>, local: Option<LocalObject>, refs: Vec<u64>` |
| `ObjectInfo` | `interface: String, methods: Vec<String>, local: Option<LocalObject>` |
| `LocalObject` | `process: u64, object: u64` |
| `InvokeResult` | `value: Value, refs: Vec<u64>` |
| `ValueResult` | `value: Value`，承载配置或导出的普通状态值 |
| `State` | `schema: String, version: u32, data: Value` |
| `ContextDerived` | `context: u64, object: u64, refs: Vec<u64>` |

### 9 宿主模块准备与退役

| 方法 | 方向、类别 | 参数 | 结果 |
|---|---|---|---|
| `library_prepare` | C→H request | `path: String` | `{library: u64, plugin: Plugin}` |
| `library_activate` | C→H request | `library: u64` | `null` |
| `library_retire` | C→H request | `library: u64` | `{leases: u64, closed: bool, quiescence_ms: Option<u64>, retired_images: u64}` |

### 11 转发通道

| 方法 | 方向、类别 | 参数字段 | 结果或含义 |
|---|---|---|---|
| `channel_open` | 转发方→核心 request | `channel: u64, metadata: Value` | `null`，建立独立逻辑连接 |
| `channel_frame` | 双向 notification | `channel: u64, frame: bytes` | `frame` 为一个完整的内层 UP/1 CBOR 消息体 |
| `channel_close` | 双向 notification | `channel: u64, reason: String` | 关闭该逻辑连接并回收其资源 |

### 13 隔离、拦截与子组件协议

| 方法或字段 | 方向 | 参数或类型 | 结果与语义 |
|---|---|---|---|
| `context_derive` | H→C request | `context: u64, isolate: Vec<{interface: String, realm: Option<String>}>, intercept: Vec<{interface: String, meta: Value}>` | `{context: u64}`；核心依次派生隔离与拦截视图 |
| `plugin` | H→C request | `context: u64, component: String, config: Value` | `{instance: u64}`；核心在该视图中挂载调用方插件自己的组件 |
| `provide.scoped` | H→C | `bool`，默认 `false` | 表明提供方支持按使用方元数据产生视图对象 |
| `cordis.scope` | 核心经 `invoke` 调用 | `args = [meta]` | 视图资源值及其 `refs`，采用普通 `InvokeResult` |

逐字段类型以[共享消息定义](../crates/cordis-up/src/payload.rs)及[现行包络与资源规则](https://github.com/Proton1917/Rust-Cordis/blob/85860d8b1fd784f6935084c611b07e12755cd7b8/docs/16-up1-protocol.md)为准。
