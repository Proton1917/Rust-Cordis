# Cordis Rust 内核

Cordis 通过显式上下文组合组件。组件声明需要的服务，在激活时登记服务、监听器和清理动作；依赖撤回或组件释放时，运行时先停用依赖方，再执行提供方的清理。实现依据《一种面向时空可组合性的编程范式》及 Cordis `packages/core` 的行为用例。

## 运行示例

在工作区根目录执行：

```bash
cargo run -p cordis --example plugins
```

完整程序位于 [examples/plugins.rs](examples/plugins.rs)。`Provider` 提供 `Database`，`Consumer` 通过 `Inject` 声明并读取它，同时登记 `Message` 监听器。程序依次展示：

1. 消费者先挂载，状态为 `Pending`。
2. 提供方激活后，消费者变为 `Active`。
3. 提供方撤回时，消费者先清理，提供方随后清理，消费者回到 `Pending`。
4. 再次提供服务后，消费者以新的依赖纪元进入 `Active`。
5. 根纤程释放全部组件后，事件、服务、组件和效应计数恢复初始值。

## 宿主和上下文

`Context::new(Arc<H>)` 接收实现 `Spawn + Clock` 的宿主。`Spawn::spawn` 持续轮询已提交任务，`Clock` 提供当前时间和异步等待。示例通过 Tokio 实现这两个接口，确定性测试使用 `cordis-testkit::TestExecutor`。

`Context` 是可克隆的一等句柄。`extend(value)` 为视图附加按类型查找的扩展值；`isolate::<K>(realm)` 改变服务键的隔离域；`intercept::<K>(meta)` 叠加访问元数据。派生视图共享内核并保留各自的解析规则。`Context`、`Fiber`、`Effect`、`Scoped<K>` 均支持跨线程传递与共享。

## 效应和释放

`ctx.effect(label, callback)` 同步执行效应体，`ctx.effect_async(label, callback)` 由宿主驱动异步效应体。回调收到 `EffectScope`，通过下列操作交回逆变换：

| 操作 | 行为 |
|---|---|
| `fx.defer(cleanup)` | 登记同步清理函数 |
| `fx.defer_async(cleanup)` | 登记异步清理函数 |
| `fx.adopt(effect)` | 将同一纤程的子效应移入当前效应 |
| `fx.is_current()` | 查询当前执行是否仍对应有效目标 |

同一效应内部按登记的相反顺序逐个完成清理。纤程的多个顶层效应按相反顺序启动释放，并共同推进。存在先后依赖的动作通过一个效应中的多个 `defer`，或通过组件的服务依赖来组织。

`Effect::dispose()` 在调用时使执行守卫失效，并返回等待释放结果的任务；重复调用共享释放状态。清理任务被宿主连续拒收时，等待者收到调度错误，内核保留待执行的 Future。宿主恢复接收后，再次显式调用 `dispose()` 会重新提交并继续已有清理。`Effect::settled()` 等待效应体结束并取得释放句柄。句柄的 `Drop` 回收句柄引用，业务清理由显式 `dispose` 或所属纤程的停用负责。

异步组件在有意义的步骤边界调用 `ctx.is_current()` 或 `fx.is_current()`。目标改变后，当前异步步骤继续结束，再回退已登记效应。长期任务通过效应登记后交由宿主运行，`apply` 在初始登记完成后返回；关闭组件时通过清理函数结束这些任务。

## 服务和依赖

服务键实现 `ServiceKey`，其中 `Value` 是服务值类型，`Meta` 是支持 `Merge` 的元数据类型，`NAME` 用于名称登记及错误信息。`Value` 可以是 `dyn Trait`，值通过 `Arc` 共享。

| 接口 | 行为 |
|---|---|
| `ctx.provide::<K>(value)` | 提供绑定并返回 `Provision<K>` |
| `ctx.provide_with_check::<K>(value, check)` | 以可用性谓词参与依赖满足性计算 |
| `ctx.get::<K>()` | 按组件声明和祖先上下文解析服务，返回 `Scoped<K>` |
| `ctx.lookup::<K>()` | 查询当前处于可用状态的绑定 |
| `provision.set(value)` | 由持有提供句柄的一方更新绑定值 |
| `provision.notify()` | 重新计算该绑定的依赖满足性 |
| `provision.dispose().await` | 撤回绑定并等待依赖方清理 |

组件通过 `Inject::new().with::<K>()` 声明依赖，或通过 `with_meta::<K>(meta)` 同时声明访问元数据。组件激活期间的授权访问读取已提交依赖视图；绑定身份改变后，纪元推动停用和重新激活。隔离域相同的视图共享绑定，使用 `Realm::new()` 的视图获得独立绑定。

`Scoped<K>` 提供对服务值的解引用访问。`caller()` 保留调用方上下文，服务方法可在该上下文登记归属调用方的效应；`definition()` 保留提供方上下文，`dependency::<D>()` 用它读取服务自身声明的依赖。`meta()` 按声明元数据、外层拦截、内层拦截的顺序合并；后加入的元数据具有相应优先级。

## 组件生命周期

静态组件实现 `Component`，声明名称、`Config`、`inject()` 和异步 `apply()`。`Config` 分别定义原始配置类型及解析结果，校验错误通过 `ValidationError` 保留消息和字段路径。闭包组件使用 `ComponentDef::new`；带转换与校验的闭包组件使用 `ComponentDef::validated`。

`ctx.mount::<C>(raw)` 和 `ctx.mount_with(&definition, raw)` 立即返回 `Fiber`，`fiber.settled().await` 等待当前及串联迁移结束。纤程有 `Pending`、`Loading`、`Active`、`Unloading`、`Failed`、`Disposed` 六种状态。

| 操作 | 行为 |
|---|---|
| `fiber.update(raw)` | 解析配置，通知更新监听器，并协调到最新目标 |
| `fiber.restart()` | 增加修订号并重新协调生命周期 |
| `fiber.dispose()` | 释放组件及其子组件 |
| `root.fiber().dispose()` | 释放根上的组件和效应，根恢复 `Active` |
| `ctx.registry().replace(&old, &new)` | 在原纤程身份上切换实现与依赖规格 |

并发更新收敛到最后一次配置。加载和清理按惯性状态运行；当前迁移结束后根据最新纪元决定下一次动作。加载失败时已登记效应被恢复，错误由 `settled()` 交给等待方；通过配置更新或实现替换重新尝试加载。

应用退出时显式执行并等待 `root.fiber().dispose()`。内核保存效应的清理闭包；闭包捕获 `Context` 时，该上下文又通过 `Arc` 持有内核，形成 `Kernel → 效应闭包 → Context → Kernel` 的强引用环。根纤程释放会执行并移出这些闭包，解除循环引用，同时让组件资源按依赖顺序完成清理。随后释放应用持有的上下文和纤程句柄，内核记录即可按引用归属回收。

## 类型化事件

每个事件实现 `Event`，给出 `NAME`、`Args`、`Output` 和 `Kind`。同一个事件类型固定一种回调族：

| `Kind` | 登记接口 | 派发接口 |
|---|---|---|
| `SyncKind` | `on`、`on_with`、`once`、`once_with` | `emit`、`bail` |
| `AsyncKind` | `on_async`、`once_async` | `parallel`、`serial` |
| `WaterfallKind` | `on_waterfall`、`once_waterfall` | `waterfall` |

同步监听器返回 `Option<Output>`。`emit` 执行全部监听器，`bail` 在首个 `Some` 处结束。异步监听器返回 `Task<Result<Option<Output>>>`；`parallel` 等待全部监听器并聚合错误，`serial` 逐个等待并在首个 `Some` 处结束。

瀑布监听器接收 `Next<E>`，通过 `next.run()` 进入下一层；直接返回值可以结束后续调用。每层延续共享一次调用权，重复调用返回错误。异步瀑布把 `Output` 定义为 `Task<Result<T>>`，通过 `next.run()?.await?` 获得下一层结果。

`EventOptions` 的 `prepend` 控制头插，`global` 控制过滤。各派发接口的 `_filtered` 变体接收监听器上下文谓词；全局监听器直接通过过滤。派发使用固定快照，执行前检查监听器的移除状态，新增登记从下一次派发生效。一次性监听器在调用用户回调前消费登记，支持递归与并发派发。

`InternalListener` 允许接管登记并返回效应，`InternalDispatch` 提供派发观察。组件更新通过 `InternalUpdate` 瀑布事件完成，局部监听器归属当前纤程并随其重载释放。

## 日志和错误

`ctx.logger()` 提供日志器，`named(name)` 指定名称。`error`、`warn`、`info`、`debug` 对应四个级别；`exporter(options, callback)` 登记可回退导出器。`buffer()` 读取按时间顺序保留的环形缓冲，`set_buffer_size(capacity)` 设置容量。导出器分别按名称及默认级别筛选消息。

`Error` 可克隆，携带消息、登记位置和聚合成员。内核驱动的组件执行与释放回调发生 panic 时转为调用失败；调用方驱动的事件回调发生 panic 时沿调用链传播。内核在调用用户代码和轮询用户任务前释放状态锁，支持回调重入。

## 验证

在工作区根目录运行项目命令：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p cordis --example plugins
cargo doc --workspace --no-deps
conda run -n base python tools/conformance.py check --through M6
```

[tests/stress.rs](tests/stress.rs) 配置 8 个 Tokio 工作线程，固定执行 1000 轮，整次运行上限为 120 秒。每轮的 8 个隔离域通过屏障并发发起配置更新和重启；每次等待返回后立即检查配置值、实际 `apply` 次数及纤程状态。随后执行依赖撤回、重供和根回收，检查消费者先于提供方清理及计数快照恢复。

语义条款位于 [docs/01-semantics.md](../../docs/01-semantics.md)，架构及接口映射位于 [docs/02-architecture.md](../../docs/02-architecture.md)，177 条上游用例的对应位置位于 [docs/04-conformance.md](../../docs/04-conformance.md)。
