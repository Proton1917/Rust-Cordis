# Rust 接口

## 组件与声明

`Component::Config` 实现 `serde::de::DeserializeOwned + Send + Sync + 'static`。空配置使用 SDK 的 `EmptyConfig`。`Component::apply(ctx, config)` 是异步函数并返回 SDK `Result<()>`；`Component::resolve(&config)` 返回配置检查结果。

`Inject::new().with::<dyn I>()` 声明必需依赖，`optional::<dyn I>()` 声明可选依赖。`Provides::new().with::<dyn I>()` 声明提供的服务，`Listens::new().with::<E>()` 声明事件监听。组件自己的声明与它实际取得、提供和监听的接口保持对应。

服务实现采用 `Arc` 共享。`Service<dyn I>` 实现接口方法的代理和克隆；同组与跨组使用同一份插件源码，调用路径由宿主选择。

## 数据与版本

接口方法返回 SDK `Result<T>`。字符串和字节的拥有型结果分别使用 `String` 与 `Vec<u8>`；复杂数据类型派生 serde 序列化和反序列化。资源参数使用 `Service<dyn I>`，由 SDK 管理跨进程引用。

接口主版本表示行为兼容边界，次版本表示追加能力。追加方法放在 trait 末尾；数据类型新增可选字段时给出 serde 缺省行为。使用方声明的次版本是它需要的最低版本。状态模式有独立的名称与版本范围，通过 `Component::state()` 声明。

## 隔离与拦截

`ctx.isolate::<dyn I>(Some("tenant-a"))` 进入命名域；相同域名共享服务集合。`None` 创建新的独有域。`ctx.intercept::<dyn I, _>(&metadata)` 叠加接口元数据。需要该元数据的提供方使用 `Scoped` 与 `provide_scoped` 创建使用方视图。

`ctx.plugin::<Child>(&config).await` 在当前视图挂载本插件描述符中声明的子组件。父实例释放完成时，子实例与其服务已回收。需要相互等待的清理通过同一效应的步骤或明确服务依赖表达。

## 事件

事件类型使用 `#[event(name = "名称", version = "1.0")]`，异步事件增加 `kind = "async"`。同步事件用 `ctx.on`、`ctx.emit`，异步事件用 `ctx.on_async`、`ctx.emit_async`。跨进程监听器收到异步通知；需要返回值的交互使用服务方法。

## kv@1 的既有接口

项目的共享 `kv-api` crate 定义 `Kv`：`get(&str) -> Result<Option<Vec<u8>>>`，`put(&str, Vec<u8>) -> Result<()>`，`keys() -> Result<Vec<String>>`，`async flush() -> Result<()>`。缺席键返回 None；put 完整替换该键的字节；keys 按字典顺序返回；flush 等待先前写入达到实现声明的持久化程度。

内存与文件实现共享 `Snapshot { entries: BTreeMap<String, Vec<u8>> }`，状态模式为 `kv.snapshot`、版本 1。写入在同一状态锁内检查 Frozen，导出器使用该锁读取快照。成功写入后派发 `Changed { key: String }`，事件名 `kv.changed@1.0`。实现该接口时依赖原 `kv-api` 定义，并让它与 cordis-sdk 使用相同仓库修订。

## 共享模板

`templates/plugin.rs` 是 `cordis new-plugin` 使用的组件源码。需要新接口和契约时，按以下布局使用完整模板，把 `{{name}}` 替换为符合命名规则的服务名：

| 目标 | manifest | 源文件 |
|---|---|---|
| interfaces/name-api | interface.toml | interface.rs → src/lib.rs |
| plugins/name | provider.toml | provider.rs → src/lib.rs |
| contracts/name-contract | contract.toml | contract.rs → src/lib.rs |

创建这些目录后把 interfaces/*、contracts/* 加入项目 workspace members，并为 cordis-contract 声明与 SDK 相同的 Git 修订或本机源码位置。provider 模板展示共享状态锁、冻结与事件，contract 模板以真实 set/value 操作检查跨阶段状态继承。
