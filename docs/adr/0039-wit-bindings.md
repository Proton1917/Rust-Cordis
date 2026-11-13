# ADR-0039 WIT 绑定、复合资源与异步值

| 项 | 内容 |
|---|---|
| 状态 | M17 实施依据 |
| 影响 | DOC-15 §7、ADR-0020；cordis-idl、cordis-dev、ABI 内部编码、Rust SDK 与宏 |

## 问题

WIT 声明需要同时生成 Rust、Python、TypeScript 的提供方接口、数据类型和调用方代理。已有 kv-api 的方法顺序、字节串调用、Snapshot 映射和 Changed 事件是兼容基准。复合值中的资源需要携带编号的拥有关系；可选值需要区分 None 与 Some(None)。

现行 WIT 定义包含 map、future、stream；wit-parser 0.263.0 提供这些类型及异步函数的已解析结构。生成器直接使用该解析器完成包、依赖、名称和类型解析。

## 选项与判据

| 选项 | 行为 |
|---|---|
| 各语言分别遍历 WIT 并决定编码 | 每个生成器分别维护方法名称、类型关系和传输表示 |
| 共用已解析类型模型与传输约定 | 各语言根据同一份接口、方法和类型信息生成语言绑定 |

采用第二项。判据是现有 Rust 插件兼容、跨语言值保持同一表示、资源引用随调用回收、运行核心维持现有协议职责。

## 解析与来源

WIT 为接口定义来源。包版本的主版本和次版本形成 Cordis 接口版本，修订号保留为来源信息。方法声明顺序进入原生方法表；兼容追加按 DOC-11 的方法表规则处理，`@since` 信息同时保留在生成描述中。

WIT 的 Markdown 文档可带一个 `cordis` TOML 代码块，表达运行时名称和事件属性。接口块可设置 `name`，例如 cordis.manage；record 块可设置 `event` 和 `kind`，例如 kv.changed 与 sync。pulldown-cmark 解析代码块，toml 解析块内属性；普通文档保留为生成接口的说明。该信息与类型共同保存在同一个 WIT 文件中。

Rust 的字符串入参生成 &str，字符串结果生成 String。方法位置展开别名，使 list<u8> 保持 Vec<u8>，继续使用 SDK 的直接字节通道。Rust、Python 的方法名采用各自有效的 snake_case，TypeScript 采用 camelCase；传输方法名统一取 Rust 有效标识符，资源构造与静态函数采用资源名称前缀。资源实例方法使用所属资源的服务对象。

## 数据表示

| WIT 类型 | Rust | Python | TypeScript | 传输表示 |
|---|---|---|---|---|
| 整数、浮点、bool、char、string | 对应标量，char 为 Unicode 标量 | int、float、bool、str | 64 位整数为 bigint，其余 number、boolean、string | 对应 CBOR 标量 |
| record | 具名 struct | dataclass | interface | 字段名映射 |
| variant / enum | enum | 具名变体 / Enum | 判别联合 / 字符串联合 | variant 为单键映射，空载荷为 null；enum 为字符串 |
| list、tuple、固定长度 list | Vec、tuple、固定长度容器 | list、tuple | Array、tuple | 数组；方法位置的 list<u8> 使用字节串 |
| map | BTreeMap | dict | Map | CBOR 映射，重复键遵循 WIT 的最后值规则 |
| flags | 带布尔字段的值 | 带布尔字段的值 | 带布尔字段的值 | 字段名到布尔值的映射 |
| option | Option | 可选值及必要的 Some 包装 | 可选值及必要的 Some 包装 | None 为 null；Some 的载荷为 null 或既有 Some 标签时，用 tag 60001 包裹 |
| result<T,E> 方法结果 | cordis_sdk::Result<T> | T 或 CordisError | Promise<T> 或 CordisError | 成功值与统一错误通道；E 进入方法描述，业务细节由 detail 承载 |
| 数据中的 result | 带 ok/err 分支的数据 | 带 ok/err 分支的数据 | 带 ok/err 分支的数据 | 单键映射 |
| resource / own | Service<dyn I> | 有类型的服务代理 | 有类型的服务代理 | tag 60000 的对象编号与既有引用管理 |
| borrow | 调用期资源借用 | 调用期借用代理 | 调用期借用代理 | 调用期保有独立引用，调用方原句柄保留 |

Serde 记录和映射内的 Vec<u8> 继续采用数组，保持 kv.snapshot 的既有状态格式。生成的字段编解码在出现可空嵌套时使用 tag 60001，保留 None 与 Some(None) 的区别；常规 SDK 状态 serde 保持原有行为。字段名、variant 分支和 enum 值使用 WIT 原名，语言标识符的调整通过生成的编码信息恢复。

方法中的可空聚合由生成的适配模块处理。参数使用 `#[cordis(with = codec_module)]`，方法使用 `#[cordis(result_with = codec_module)]`。模块的 `Encode(&T)` 实现 Serialize，`Decode(T)` 实现 Deserialize，同时提供字段 `serde(with)` 使用的 serialize/deserialize 函数。宏保留公开 Rust 类型，通用入口将完整值交给适配模块。类型化入口按原有 Option 链拆分或重建 CborValue，维持 AbiOption 与 AbiBuf 的函数指针签名。适配目标是以 Cbor 聚合结束的 Option 链，直接标量和字节串的 Option 链沿现有宏处理。适配模块按所属位置保留方法字节串与记录数组的差异。

## 调用期资源编码

ABI 内部编码器提供以下安全接口，SDK 的 Cbor 路径使用这些接口：

| 接口 | 作用 |
|---|---|
| CborEncoder::value(&mut self, &T) | 在当前编码器的同步作用域中将 Serialize 值转换为 CborValue；失败回收本轮导出并保留先前成功值 |
| CborEncoder::encode(self, &T) | 编码完整值，将导出编号的拥有者交给 AbiBuf owner |
| from_value_in(value, Environment) | 在接收环境中反序列化一个值，导入资源引用 |
| decode_in(bytes, Environment) | 以既有 CBOR 库读取字节并在接收环境中反序列化 |

Service<I> 实现 Serialize、Deserialize 及基于接口身份的 Debug。Serialize 在活动作用域中导出 tag 60000；Deserialize 使用 Environment.host().import_resource 取得独立引用。记录、列表、映射允许的位置、tuple、option 与 variant 由 serde 递归使用相同作用域。

Context::service<I>(Arc<I>) 创建可返回的局部服务对象，复用 Interface::object 和 Service::from_object。资源构造器、future 与 stream 的提供方由此交出实例的资源引用。

作用域采用 scoped-tls 的借用式线程局部槽，作用域值留在同步调用栈上，RAII 守卫恢复上一层作用域。槽自身具有无析构表示，使 WhenUnleased 的库卸载与线程随后退出符合 ADR-0012 的生命周期要求。编码器的导出编号集合由 Arc 拥有，使 Encoder 和 AbiBuf 保持 Send；编号集合的锁只用于更新集合，Host 导入和导出在锁外执行。作用域结束后，已创建缓冲继续持有导出引用；缓冲释放、序列化失败、反序列化失败与异步取消通过既有资源释放路径回收。

普通 encode/decode/to_value/from_value 显式隔离调用期资源作用域，用于状态持久化与普通数据；其中出现 Service 时返回需要调用期环境的错误。生成字段的 serde helper 直接使用 CborValue::serialized/deserialized 继承其外层作用域。类型化调用中的 Cbor 参数和返回值、通用调用的 Cbor 编码和解码接入上述接口。primitive、String、Vec<u8> 的现有快路保持，AbiBuf 布局与 Host 槽保持 v1。

借用资源的 Rust 绑定使用 BorrowedService<'a,I>：调用方通过 Service.borrow() 保留原句柄与借用生命周期；包装 Deref 到接口 I，Clone、Debug 和 serde 保留调用期身份。接收侧反序列化为本次调用取得引用，异步调用保有该引用至完成或取消。复合输入中的借用信息由已解析类型模型传递到生成类型。

事件的发送和监听使用同一调用期资源编解码。ABI async_callback 在 call_async 边界构造回调 Future，使 on_async 在原输入载荷释放前完成 decode_in；Future 继续持有 callback Arc，监听器的异步主体在 poll 中执行。emit_async 在 Host 接管输入引用后释放编码缓冲。ABI 布局和 Host 槽保持原值。

## future、stream 与脚本调用方

future<T> 生成具有异步 resolve()->Result<T> 的资源服务接口；stream<T> 生成具有异步 next()->Result<Option<T>> 的资源服务接口，None 表示流结束。接口身份由完整的载荷类型名称形成，匿名组合采用结构名称，使相同类型在参数和返回值中使用同一服务身份。资源的关闭和取消沿已有对象引用与调用取消协议执行。

Python 与 TypeScript 分别生成提供方接口和远程 Client。提供方声明保留 WIT 的同步、异步区别；远程 Client 返回 awaitable/Promise，并把源方法的异步标记交给调用传输，以便 Rust 宿主选择相应入口。数据编解码与资源绑定采用明确的调用接口，后续语言 SDK 复用这套接口。

## 模块与依赖

解析模型和三语言输出进入独立开发库 cordis-idl，目标 2500 代码行、上限 4000。cordis-dev 的 idl gen 负责命令参数、输入和输出；core 进程继续使用现有 ABI/UP1 数据定义。生成结果保留为普通源码，kv-api 的构建使用已生成 Rust，Rust 1.85 的插件构建沿用既有依赖。

开发库使用 wit-parser 0.263.0、heck 0.5.0、pulldown-cmark 0.13.6、toml、serde_json，以及 syn/prettyplease 校验并格式化 Rust。这些依赖位于开发工具的依赖图。SDK 资源编码使用 scoped-tls 1.0.1、既有 serde/ciborium 与资源生命周期机制。

Rust 生成模块采用 prettyplease 的确定布局，并以 rustfmt::skip 保持该布局；开发库源码继续使用工作区的 rustfmt。生成模块的命名空间以双下划线区分包、接口与资源静态方法，non_snake_case 属性对应这一名称规则。

## 验收与后果

DOC-15 §7 每行都有明确用例；真实 KV 的生成接口与现有实现一起构建并通过 kv-contract。复合资源用例覆盖 record/list/option 的参数和返回、成功、失败与取消后的引用回收。生成字段的嵌套 option 以独立期望值检查 null 与 tag 60001 的表示。future 和 stream 通过真实资源方法取得值、结束及释放。

三语言共享同一份定义和传输约定，语言特有代码负责相应对象与调用形式。M17 的宏职责拆分与新增资源编解码分别保留变更来源，按最终接口组合验证；核心 ABI 布局和协议版本维持现有值。

依据：WebAssembly/component-model 的 [WIT 规范](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)、wit-parser 0.263.0 的 TypeDefKind/FunctionKind/Resolve 定义、DOC-12 §5/§7/§8、ADR-0020。

## 修订（M22，设计方 2026-11-13）：`list<u8>` 在任何位置都编码为 CBOR 字节串

问题：本 ADR“数据表示”一节规定，方法位置的 `list<u8>` 编码为字节串，记录、变体、map、option、result 等聚合内部的 `list<u8>` 编码为整数数组。同一个 WIT 类型因位置不同而有两种表示。整数数组受 UP/1 `max_collection = 65536` 约束，所以聚合内部的字节字段超过 64 KiB 时调用失败；每个字节占一个数组元素，体积约为字节串的两倍。M22 的 HTTP 内联体（ADR-0046 §3.1，缺省 1 MiB）首先撞到这个上限。任何在结构中携带字节的接口都会遇到同一问题。

选项：

| 选项 | 内容 | 判断 |
|---|---|---|
| 标注 | 在 WIT 别名上加 `cordis bytes=true`，仅对标注处使用字节串 | 保留两种表示；接口作者遗漏标注即在 64 KiB 处失败 |
| 统一 | `list<u8>` 在任何位置都编码为字节串 | 一种类型一种表示；在首次发布之前修改，没有外部使用方 |

决定：采用统一。

1. 三种语言的生成器与 Rust SDK 在写出时，`list<u8>` 在任何位置都编码为 CBOR 字节串（主类型 2）。
2. 读取时同时接受字节串与整数数组，这样 M17–M21 写出的检查点、状态快照（例如 kv 的 `Snapshot.entries`）仍可读取。整数数组仍受 `max_collection` 约束。v0.x 期间保持双读，移除时另立 ADR。
3. Rust SDK 提供 `cordis_sdk::Bytes`（或等价的字段编解码属性），实现上述写出与双读。生成的 Rust 代码在聚合内的 `list<u8>` 字段上使用它。手写 `#[cordis_sdk::interface]` 接口的结构体字段，需要字节串表示时使用同一类型；guide 与 skill 写明这一点。
4. 核心、ABI、UP/1 的预算与协议版本不变。核心转交的载荷不透明，同组 typed 直接调用不经过 CBOR，均不受影响。
5. 验收：
   - 每种语言各有用例，覆盖记录、变体、map、option、result 中的 1 MiB `list<u8>` 往返；
   - 用 M21 写出的 kv 快照做跨语言状态继承，读取成功；
   - kv-contract 对三种语言实现全部通过；
   - 跨语言迁移（AU-5）与 M21 的跨语言测试全部通过。
