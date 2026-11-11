# 02 Rust 架构

| 项 | 内容 |
|---|---|
| 文档编号 | `DOC-02` |
| 状态 | 架构规则 `A-*` 已定稿，修订需写 ADR。§6 记录 M6 的公开接口形状，完整可运行程序见 `crates/cordis/examples/`；接口语义由 §7 映射表约束 |
| 读者 | 实现者、评审者 |
| 输入 | `DOC-00` 的公理 `P1`–`P10`；`DOC-01` 的条款 `T-*`、`S-*`；`ADR-0001`–`ADR-0006` |
| 输出 | 仓库布局、模块划分、数据模型、架构规则 `A-1`–`A-12`、接口草图、`UPSTREAM` 到 Rust 的映射表、依赖白名单 |
| 不包含 | 排期与退出条件（见 `DOC-03`） |

## 1 仓库布局

工作区根目录是 `~/Projects/Rust-cordis/`。

```
Rust-cordis/
├── AGENTS.md                 实现者入口
├── Cargo.toml                cargo workspace
├── rust-toolchain.toml       固定工具链（当前机器为 1.96.0 stable）
├── crates/
│   ├── cordis/               内核。唯一的产品 crate
│   └── cordis-testkit/       确定性执行器、假时钟、快照工具。publish = false
├── docs/                     DOC-00 … DOC-04、adr/、PROGRESS.md、reference/
├── tools/conformance.py      一致性矩阵工具
└── cordis/                   UPSTREAM 只读克隆（commit 261c5fd），不纳入本仓库版本控制
```

`cordis/` 与 `docs/reference/` 写入 `.gitignore`。`cordis/` 内的任何文件不得修改。

## 2 模块划分

内核 crate 的模块与 `UPSTREAM` 的源文件对应，便于逐文件对照阅读。

| Rust 模块（`crates/cordis/src/`） | 对应 `UPSTREAM` | 职责 | 实现的条款 |
|---|---|---|---|
| `kernel.rs` | 无直接对应（JS 的单线程事件循环与垃圾回收） | 内核锁、arena、调和队列、`Spawn` 与 `Clock` 的接入 | `A-1`–`A-6` |
| `context.rs` | `context.ts` | `Context` 句柄与不可变视图链；`extend`、`isolate`、`intercept` | `T-COE-7`–`T-COE-13`、`S-CTX-*` |
| `effect.rs` | `fiber.ts:240-363` | `effect`、效应作用域、释放函数、幂等防护、元信息树 | `T-EFF-*`、`S-EFF-*` |
| `fiber.rs` | `fiber.ts` 其余部分 | 纤程记录、状态、纪元、`reload`/`unload`、`restart`、`update` | `T-LIF-*`、`S-FBR-*` |
| `reflect.rs` | `reflect.ts` | 绑定存储、`provide`、`get`、`locate`、`notify` | `T-COE-1`–`T-COE-6`、`T-CTX-2`、`S-SVC-1`–`S-SVC-10` |
| `service.rs` | `service.ts` | `ServiceKey`、`Scoped<K>`、拦截元数据的合并与解析 | `T-COE-9`–`T-COE-11`、`S-SVC-11`–`S-SVC-13` |
| `registry.rs` | `registry.ts` | 组件定义、运行时记录、`mount`、`inject`、`replace`、配置校验 | `S-REG-*` |
| `events.rs` | `events.ts` | 监听器表、五种派发、内核事件 | `S-EVT-*` |
| `logger.rs` | `logger.ts` | 日志服务、导出器、环形缓冲 | `S-LOG-*` |
| `error.rs` | `utils.ts:248-298`、`fiber.ts:16-111` | 错误类型、调用位置记录 | `S-FBR-3`、`S-REG-6`、`S-SVC-7` |
| `list.rs` | `utils.ts:8-53` | `DisposableList`：支持头插、尾插、按句柄删除的有序表 | `S-EFF-6` |

`utils.ts:126-246` 的可追踪代理没有对应模块，其语义由 `Scoped<K>` 承担（§5.3）。

## 3 数据模型

### 3.1 内核状态

`UPSTREAM` 的对象图含有环：纤程指向上下文，上下文指向纤程，绑定指向纤程集合。Rust 用 arena 加代际 ID 表示这个图，句柄只持有 ID。

```rust
pub struct Kernel {
    state: Mutex<State>,          // 唯一的内核锁
    spawner: Arc<dyn Spawn>,
    clock: Arc<dyn Clock>,
}

struct State {
    fibers:   SlotMap<FiberId, FiberData>,
    impls:    SlotMap<ImplId, ImplData>,
    store:    HashMap<(KeyId, RealmId), ImplId>,     // σ : R ⇀ V，按键与域索引
    runtimes: HashMap<ComponentId, RuntimeData>,
    aliases:  HashMap<ComponentId, ComponentDef>,
    hooks:    HashMap<EventId, Vec<Arc<Hook>>>,
    names:    HashMap<&'static str, KeyId>,          // 名称登记表（ADR-0004）
    dirty:    VecDeque<Arc<Scope>>,                  // 调和队列持有所属视图，Scope 携带 FiberId
    draining: bool,
    counters: Counters,                              // fiber uid、impl uid、日志序号
}
```

`FiberData` 的字段与 `DOC-01` §2.2 的字段表一一对应。`ImplData` 的字段与 `DOC-01` §2.3 的 `Impl` 一一对应。

### 3.2 上下文视图链

`UPSTREAM` 的派生上下文通过原型链继承父上下文。Rust 用不可变的持久链表表示同一结构。节点一经创建不再修改，因此读取视图链不需要内核锁；节点由 `Arc` 计数回收，对应 `T-COE-13` 的“恢复即丢弃子上下文”。

```rust
#[derive(Clone)]
pub struct Context {
    kernel: Arc<Kernel>,
    scope:  Arc<Scope>,
}

struct Scope {
    parent: Option<Arc<Scope>>,
    fiber:  FiberId,                     // 最近的所属纤程
    layer:  Layer,
    lease: Option<Weak<Kernel>>,         // 纤程层持有记录回收租约，见 ADR-0007
}

enum Layer {
    Root,
    Fiber,                                           // 纤程自己的上下文
    Isolate   { key: KeyId, realm: RealmId },        // ρ 的一处覆盖
    Intercept { key: KeyId, meta: Arc<dyn Any + Send + Sync> },   // ι 的一层
}
```

解析 `ρ(k)`：从当前节点向根查找第一个 `Isolate { key: k, .. }`，找不到时取默认域 `RealmId::DEFAULT`（`S-CTX-5`）。解析 `ι(k)`：自根向当前节点收集全部 `Intercept { key: k, .. }` 层（`S-SVC-11`）。

`Fiber` 持有所属纤程层的 `Arc<Scope>`。活跃记录保留该层；释放稳定后取出记录中的强引用，相关上下文与句柄释放完毕时由租约回收 arena 记录。任务、依赖释放及失活上下文的访问共用这一生命周期（ADR-0007）。

### 3.3 纪元

```rust
enum Epoch {
    Inactive,                                        // ⊥
    Active { revision: u64, impls: Vec<u64> },       // 各已声明键的绑定 uid，按声明顺序
}
```

相等比较即 `T-LIF-2` 的纪元相等。

## 4 架构规则

规则是硬约束。评审时逐条检查。

### 并发与重入

| 规则 | 内容 | 依据 |
|---|---|---|
| `A-1` | 全部可变内核状态位于 `State` 中，由 `Kernel::state` 这一把锁保护。不设第二把锁，不使用 `unsafe` | `P7`、`ADR-0002` |
| `A-2` | 对外句柄 `Context`、`Fiber`、`Scoped<K>`、`Effect` 都是 `Clone + Send + Sync`。用户回调的约束是 `Send + Sync + 'static`，用户 `Future` 的约束是 `Send + 'static` | `G4` |
| `A-3` | 持有内核锁期间不调用任何用户代码，不轮询任何用户 `Future`，不 `await`。每个内核操作的形态是：加锁，读写状态，收集待办动作（要调用的回调、要派发的事件、要生成的任务、要唤醒的等待者），解锁，执行待办动作 | `P7`、`R1` |
| `A-4` | 事件派发在锁内对监听器列表取快照（`Vec<Arc<Hook>>` 的克隆），在锁外依次调用。调用每个监听器之前检查其 `removed` 原子标志 | `S-EVT-2` |
| `A-5` | 纤程状态迁移由单一调和循环串行执行。改变某纤程目标纪元的操作把该纤程放入 `dirty` 队列；当前没有调和循环在运行时，发起操作的线程成为调和者并处理队列直到为空；已有调和者时只入队。调和循环在处理相邻两个纤程之间释放锁 | `P3`、`ADR-0002` |
| `A-6` | `reload` 与 `unload` 各是一个通过 `Spawn` 生成的任务，其句柄即 `inertia`。一个纤程在任一时刻至多有一个这样的任务 | `T-LIF-6`、`P9` |

### 生命周期与错误

| 规则 | 内容 | 依据 |
|---|---|---|
| `A-7` | 不通过丢弃 `Future` 取消组件的加载。被取代的加载运行到下一个步骤边界或运行结束，随后其已登记的效应被回退 | `P4`、`ADR-0005` |
| `A-8` | 清理不依赖 `Drop`。释放函数只由 `Effect::dispose` 或内核的 `unload` 调用。句柄被丢弃时效应继续存在，直到所属纤程卸载 | `R2` |
| `A-9` | 内核驱动的用户代码（组件的效应函数、释放函数）发生 panic 时，内核捕获它并按该次调用失败处理。调用方驱动的用户代码（`emit` 的监听器等）的 panic 向调用方传播。两种情况下内核状态都保持一致，因为规则 `A-3` 保证此时不持锁 | `P8` |
| `A-10` | 交给多个等待者的错误可克隆。错误携带登记位置：`effect`、`mount`、`provide`、`on` 用 `#[track_caller]` 记录调用位置，取代 `UPSTREAM` 的堆栈拼接 | `S-FBR-21`、`utils.ts:253-298` |

### 规模

| 规则 | 内容 | 依据 |
|---|---|---|
| `A-11` | 内核 crate 的运行时依赖只取自 §9 的白名单。新增依赖先写 ADR | `G5`、`P10` |
| `A-12` | 内核 crate 不依赖 tokio、不依赖任何具体执行器。`cordis-testkit` 与测试可以依赖 | `P9` |

## 5 关键机制

### 5.1 调和循环

本节说明规则 `A-5` 如何实现 `PAPER` 算法 5。

1. 输入：`dirty` 队列中的纤程 ID。入队的来源有五个：纤程实例化、`notify`、`restart`、`replace`、释放。
2. 对每个出队的纤程，在锁内按 `S-FBR-5` 计算纪元，按 `S-FBR-6` 判定。
3. 判定结果是“启动迁移”时，在锁内写入新状态与 `inertia` 占位，把“生成任务”和“派发 `internal/status`”记入待办动作。
4. 解锁，执行待办动作。
5. 迁移任务结束时再次加锁，按 `S-FBR-9` 或 `S-FBR-12` 的末尾步骤判定：清除 `inertia` 并唤醒等待者，或原地串联进入反向迁移。
6. 验证：`UPD-02`–`UPD-07`、`UPD-09`、`UPD-10`（里程碑 `M3`）；`FBR-01`–`FBR-03`（inertia lock）、`UPD-01`、`UPD-08`（里程碑 `M4`，场景含依赖变化）。

纤程的 `queued` 标志与修订号、存活状态或依赖视图的变更在同一次持锁中更新；入队同步取得所属 Scope。稳定判定要求队列、迁移、状态通知及保留的清理任务全部结束。状态变更同步登记通知计数，计数在 `InternalStatus` 与服务依赖通知完成后释放；迁移在通知返回后重新判断目标纪元。保持当前目标的出队分支也会唤醒等待者。队列动作及迁移任务继续持有同一个 Scope，记录回收据此遵循实际任务生命周期。

状态变化跨越 `ACTIVE` 边界时（`S-FBR-2`），受影响的声明方纤程在同一把锁内入队，由同一个调和循环继续处理。这给出了 `T-COE-6` 的依赖顺序。

### 5.2 效应作用域

`UPSTREAM` 用生成器的 `yield` 表达效应迭代器的步骤。Rust 没有稳定的生成器，步骤由效应作用域上的显式调用表达：每次 `defer` 交回一个逆变换，相当于一次 `yield`。

| `PAPER` / `UPSTREAM` | Rust |
|---|---|
| 回调返回一个释放函数 | 回调内调用一次 `fx.defer(..)` |
| 生成器每次 `yield` 一个释放函数 | 回调内多次调用 `fx.defer(..)` |
| `yield` 另一个效应的释放函数 | `fx.adopt(effect)`：把子效应移到本效应名下（`S-EFF-6`） |
| 步骤边界的守卫检查 | `fx.is_current()`；异步回调在每个 `await` 之后检查它并在失效时提前返回 |
| 回调抛错 | 回调返回 `Err`；已 `defer` 的部分按相反顺序释放（`S-EFF-2`） |

释放函数有同步与异步两种形态：`fx.defer(|| ..)` 与 `fx.defer_async(|| async { .. })`。

### 5.3 调用方的显式传递

`UPSTREAM` 的可追踪代理解决三个问题。Rust 用 `Scoped<K>` 逐一对应。

| 问题 | `UPSTREAM` 的做法 | Rust 的做法 |
|---|---|---|
| 服务方法登记的效应归属于调用方纤程 | 代理把 `this.ctx` 替换为使用点上下文 | `Scoped<K>` 持有调用方 `Context`；服务方法从 `scoped.caller()` 取上下文并在其上调用 `effect` |
| 服务访问自己的依赖时按服务的规格解析 | 代理记录定义点（shadow） | 服务在构造时保存自己的 `Context`，访问依赖时使用它 |
| 拦截元数据按调用方解析 | `resolveIntercept` 读取 `@@caller` | `scoped.meta()` 以调用方 `Context` 执行 `S-SVC-11` |

`Scoped<K>` 对 `K::Value` 实现 `Deref`。服务作者把需要调用方的方法写成 `Scoped<K>` 上的扩展 trait。

`caller()` 保留原使用点，供效应登记和拦截解析；`definition()` 指向服务提供点，供其自身依赖解析；`requester()` 指向发起当前访问的代码定义点，供声明元数据解析。`dependency::<D>()` 取得定义点中的依赖并继续携带原使用点。`meta()` 合并 requester 的声明元数据与 caller 的拦截层，`logger()` 采用当前服务来源及原使用点的日志拦截。`with_value` 创建保留上述上下文的派生服务值句柄（ADR-0008）。

### 5.4 确定性执行器

`cordis-testkit` 提供 `TestExecutor`，它实现 `Spawn` 与 `Clock`。

- 任务进入先进先出队列，在调用 `run_until_stalled()` 的线程上依次轮询。
- 时间只在调用 `advance(duration)` 时前进。
- `UPSTREAM` 用例中的 `await sleep()`、`await setImmediate()`、`vi.advanceTimersByTime(n)` 分别对应 `run_until_stalled()`、`run_until_stalled()`、`advance(n)` 后接 `run_until_stalled()`。

用例在这个执行器上断言时序。`M6` 的压力测试改用多线程执行器，只断言收敛性质（`R3`）。

## 6 公开接口

本节展示公开接口的形状。完整接口文档由 `cargo doc --workspace --no-deps` 生成，完整可运行程序位于 `crates/cordis/examples/plugins.rs` 和 `crates/cordis/examples/readme.rs`。

```rust
// ── 根 ────────────────────────────────────────────────────────────
let root: Context = Context::new(executor.clone());

// ── 可回退效应（T-EFF-*）────────────────────────────────────────────
let effect: Effect = ctx.effect("open-connection", |fx| {
    let conn = open()?;
    fx.defer(move || conn.close())?;
    Ok(())
})?;
effect.dispose().await?;           // 幂等；释放错误经 Result 返回

// ── 余效应（T-COE-*）───────────────────────────────────────────────
pub struct Database;
impl ServiceKey for Database {
    const NAME: &'static str = "database";
    type Value = dyn DatabaseApi;                   // V_k
    type Meta = DatabaseMeta;                       // M_k，实现 Merge
}

let provision = ctx.provide::<Database>(Arc::new(Sqlite::open()?))?;   // 一次 effect
let db: Scoped<Database> = ctx.get::<Database>()?;                     // 经 locate 中介
let sandbox = ctx.isolate::<Database>(Realm::new());                   // 派生上下文
let readonly = ctx.intercept::<Database>(DatabaseMeta { readonly: true });

// ── 组件（T-LIF-*）─────────────────────────────────────────────────
pub struct Consumer;
impl Component for Consumer {
    const NAME: &'static str = "consumer";
    type Config = ConsumerConfig;
    fn inject() -> Inject {                         // 余效应规格 d
        Inject::new().with::<Database>()
    }
    async fn apply(ctx: Context, config: ConsumerConfig) -> Result<()> {   // 效应函数 e
        let db = ctx.get::<Database>()?;
        ctx.on::<Message>(move |msg| { db.insert(msg); None })?;
        Ok(())
    }
}

let fiber: Fiber = ctx.mount::<Consumer>(config)?;  // 立即返回
fiber.settled().await?;                             // 等待迁移结束；失败时返回错误
fiber.update(new_config).await?;
fiber.restart().await?;
fiber.dispose().await;
assert_eq!(fiber.state(), FiberState::Disposed);

// 匿名组件，对应 UPSTREAM 的 ctx.inject(deps, callback)
ctx.inject(Inject::new().with::<Database>(), |ctx| async move { /* … */ Ok(()) })?;

// ── 事件（S-EVT-*）─────────────────────────────────────────────────
ctx.on::<Message>(|msg| { /* … */ None })?;         // 一次 effect
ctx.emit::<Message>(&msg);
```

`use` 是 Rust 关键字，`UPSTREAM` 的 `ctx.use()` 在 Rust 中命名为 `ctx.mount()`。

同步效应体、释放回调分别返回 `Result<()>`；`EffectScope::defer` 与 `adopt` 返回登记结果。`Context::extend<T>(value)` 以类型标识扩展字段，`extension::<T>()` 沿视图链读取最近值。宿主通过 `Spawn::spawn(Task)`、`Clock::now()` 与 `Clock::sleep(Duration)` 接入。

`effect_async(label, callback)` 立即登记效应并提交宿主任务。`Effect::settled()` 等待效应体及失败恢复，返回释放句柄；`defer_async` 接收返回 `Future<Output = Result<()>>` 的闭包。组件在 `Context::is_current()`、效应体在 `EffectScope::is_current()` 查询当前步骤对应的纪元。

`Component::Config: Config` 通过关联类型 `Raw` 声明原始配置，`resolve(&Raw)` 返回解析配置或 `ValidationError`；错误中的 `issues` 保留路径。动态组合使用可复用的 `ComponentDef::new/validated` 与 `mount_with`，配置在内部以 `RawConfig = Arc<dyn Any + Send + Sync>` 保存。`Fiber::settled()`、`update`、`restart`、`dispose` 返回 `Task<Result<()>>`。

事件通过 `Event::Kind` 选定 `SyncKind`、`AsyncKind` 或 `WaterfallKind`。同步登记使用 `on/once`，异步登记使用 `on_async/once_async`，瀑布登记使用 `on_waterfall/once_waterfall`；后两组同时接收 `EventOptions`。`Next::run` 返回 `Result`，异步瀑布的 `Output` 为 `Task<Result<T>>`。派发签名与过滤参数见 ADR-0006。

`Registry::replace(&old, &new)` 返回原地迁移的纤程列表，调用方通过各纤程的 `settled()` 等待结果。旧组件定义继续经别名解析到新定义。日志接口为 `ctx.logger()`，支持 `named`、四级记录、`exporter` 和 `buffer`；默认缓冲容量 1000 条，`set_buffer_size` 可调整容量。错误记录与公开缓冲共用同一数据源。

## 7 `UPSTREAM` 到 Rust 的映射

下表每一行的语义是约束。前三列取自 `PAPER` 表 1，并补充了内核的其余接口。

| `PAPER` | `UPSTREAM` | Rust | 说明 |
|---|---|---|---|
| `Γ∞` | `ctx` | `Context` | 一等上下文，`Clone + Send + Sync` |
| `effect^iter(e)` | `ctx.effect(callback)` | `ctx.effect(label, \|fx\| ..)`、`ctx.effect_async(..)` | §5.2 |
| `𝔈`、`𝔈^iter` | 返回或逐次产出逆变换的回调 | `fx.defer`、`fx.defer_async`、`fx.adopt` | §5.2 |
| `idem` | `armed` 标志 | `Effect::dispose` 内的原子标志 | `T-EFF-2` |
| `Σ`、`Σ^iso`、`Σ^inter` | `ctx[@@store]`、`ctx[@@isolate]`、`ctx[@@intercept]` | `State::store`、`Layer::Isolate`、`Layer::Intercept` | §3.1、§3.2 |
| `get(k)` | `ctx.get(key)`、属性访问 `ctx.foo` | `ctx.get::<K>() -> Result<Scoped<K>>` | 中介点，执行 `S-SVC-6` |
| `set(k, v)` | `ctx.set(key, value)`、`ctx.provide(name, value)` | `ctx.provide::<K>(value) -> Result<Provision<K>>` | `Provision<K>::set` 对应 `S-SVC-10`；所有权保证只有提供方能改值 |
| `isolate(k, r)` | `ctx.isolate(key, realm)` | `ctx.isolate::<K>(realm)` | 派生实现 |
| `intercept(k, ν)` | `ctx.intercept(key, metadata)` | `ctx.intercept::<K>(meta)` | 派生实现 |
| `⊕_k` | `Service[@@mergeIntercept]` | `K::Meta: Merge` | 默认实现为后者覆盖前者 |
| `ℭ` | `fiber` | `Fiber` | 句柄 |
| `d` | `fiber.inject`、`component.inject` | `Inject`、`Component::inject()` | 键到声明元数据的有序映射 |
| `e` | `fiber.apply`、`component.apply` | `Component::apply` | 异步函数 |
| `ε_d(σ)` | `fiber.epoch` | `Epoch` | §3.3 |
| `recover` | `fiber.dispose` | `Fiber::dispose` | |
| `notify` | `reflect.notify` | `reflect::notify` | 内部函数 |
| `refresh`、`reload`、`unload` | `_refresh`、`_reload`、`_unload` | 同名内部函数 | §5.1 |
| `resolve` | `ReflectService.handler.get`、`locate` | `reflect::locate` | 内部函数 |
| 无 | `ctx.use(component, config)` | `ctx.mount::<C>(config)` | `use` 是关键字 |
| 无 | `ctx.inject(deps, callback)` | `ctx.inject(inject, callback)` | 匿名组件 |
| 无 | `await fiber` | `fiber.settled().await` | `S-FBR-14` |
| 无 | `registry.replace(old, new)` | `ctx.registry().replace(old, new)` | `S-REG-4`、`S-REG-5` |
| 无 | 函数身份作为组件键 | `ComponentId`：类型组件取 `TypeId`，闭包组件取创建时分配的编号 | `S-REG-1` |
| 无 | `Component.Config`（Standard Schema） | `Component::Config: Config`，`Config::resolve(raw) -> Result<Self, ValidationError>` | `S-REG-6` |
| 无 | `ctx.on` / `once` / `emit` / `parallel` / `serial` / `bail` / `waterfall` | 同名方法，事件由类型 `E: Event` 标识 | `ADR-0006` |
| 无 | 派发时的 `thisArg[Context.filter]` | 派发方法的带过滤器变体 | `S-EVT-3` |
| 无 | `Service` 基类 | 普通类型加 `ServiceKey`；在 `Component::apply` 中调用 `ctx.provide` | `S-SVC-13` |
| 无 | 可追踪代理、`@@caller`、`@@shadow` | `Scoped<K>` | §5.3 |
| 无 | `ctx.mixin`、`ctx.accessor`、`ctx.foo.bar` 关联访问 | `Context` 或 `Scoped<K>` 上的扩展 trait | 编译期完成，无运行时对应物 |
| 无 | 可调用服务（`@@invoke`） | `Scoped<K>` 上的普通方法 | |
| 无 | `@Inject` 装饰器 | `Component::inject()` | 过程宏在范围外 |
| 无 | `ctx.logger` | `ctx.logger()`、`ctx.logger().named(..)` | `S-LOG-*` |
| 无 | `composeError`、`buildOuterStack` | `#[track_caller]` 与错误中的位置链 | `A-10` |
| 无 | `Context.is`、`inspect` | `Debug` 实现，输出 `Context <name>` | |

## 8 验收示例

`crates/cordis/examples/plugins.rs` 是 `G3` 的验收物，在里程碑 `M6` 交付。它包含两个组件：

- `Provider`：在 `apply` 中提供服务 `Database`。
- `Consumer`：规格声明 `Database`；在 `apply` 中读取它并登记一个事件监听器。

示例依次执行并打印每一步的纤程状态：

1. 先挂载 `Consumer`。输出其状态为 `Pending`（`T-LIF-1`：规格未满足）。
2. 挂载 `Provider`。输出 `Consumer` 的状态变为 `Active`（`T-COE-5` 激活型）。
3. 释放 `Provider`。输出 `Consumer` 先完成卸载，`Provider` 的清理随后执行（`T-COE-6`），`Consumer` 回到 `Pending`。
4. 再次挂载 `Provider`。输出 `Consumer` 重新 `Active`，其纪元与第 2 步不同（`T-LIF-2`）。
5. 释放根上的全部组件。输出事件表、服务表、组件表的快照与第 1 步之前相同（`P2`）。

## 9 依赖白名单

版本在 `M0` 选定当时的最新稳定版，记入 `docs/PROGRESS.md`。

| crate | 用于 | 范围 |
|---|---|---|
| `slotmap` | 代际 arena | 内核 |
| `parking_lot` | 内核锁（无中毒语义） | 内核 |
| `futures-core`、`futures-util` | `BoxFuture`、`FutureExt::catch_unwind`、`join_all` | 内核 |
| `thiserror` | 错误类型派生 | 内核 |
| `serde`、`serde_json` | 以 JSON 值为原始配置的 `Config` 实现 | 内核，可选特性 `json` |
| `tokio` | 多线程压力测试 | 仅 `dev-dependencies` |
