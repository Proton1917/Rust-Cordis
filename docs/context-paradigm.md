# Rust-Cordis 的上下文范式

Rust-Cordis 通过一个通用组件内核组合业务能力。组件声明所需服务，以当前加载代的上下文取得这些服务，并把自身取得的资源登记为可回收效应。新增业务能力由新的组件定义、服务契约和配置表达；已有组件通过同一套依赖解析、生命周期和资源回收规则参与组合。

本文将论文 *A Programming Paradigm for Spatiotemporal Composability* 的上下文范式对应到本仓库的 Rust 实现。论文作者为 Yifan Shi、Wei Zhang、Tianyi Cui；引用版本共 48 页，输入文件 `cordis-paper(1).pdf` 的 SHA-256 为 `57b92fa4a9e554df369aa16b8df857113a09aa3f01ec4f4033065ef972efcdb1`。页码采用论文印刷页码。定义、命题、实现投影与必要前提分别列出，供源码修改和证明复核使用。

## 组件、内核与执行位置

`ComponentDef` 保存组件身份、名称、有限依赖规格、配置解析器和加载回调。`Context::mount_with` 通过统一入口创建 Fiber，父级挂载效应拥有子 Fiber。`reflect::epoch_ids` 按声明顺序生成依赖身份向量；`fiber` 模块依据该向量、修订号及当前迁移状态决定何时加载或清理。组件名称和业务回调的具体实现参与业务执行，生命周期决策继续由这些通用事实决定。

| 层次 | 本项目的实现 | 组合契约 |
|---|---|---|
| 组件内核 | `crates/cordis` 的 Context、Fiber、Effect、服务表及 Registry | 相同的依赖、纪元、准入、父子所有权和清理规则 |
| 框架组件 | 监管器 `Manager::new` 挂载 supervisor；网页部署为每个 Owner 构造 `ComponentDef` | 通过声明依赖、派生视图和 Effect 持有框架资源 |
| 部署插件 | 原生宿主、TypeScript/Python SDK 及其部署定义 | 监管器把当前代的配置、依赖和元数据送入 UP/1 `apply` 请求，宿主执行业务代码 |
| 运行环境 | Rust 执行器、传输、文件系统、动态加载器 | 为组件运行提供各自公开的执行和资源契约 |

内核库实际调用通用 Rust 组件的加载闭包；监管器的闭包把部署插件调用交给相应连接。这里的执行位置由实际装配链确定。“核心之外一切皆插件”约束业务能力的扩展与组合方式，框架装配也复用组件机制。`PluginPrinciple.Definition` 将实现保留为任意类型参数，证明只读取依赖边界所需事实；源码证书检查实际回调装配及 RPC 参数来源。

`PluginPrinciple` 对任意有限操作迹证明：每个实例的投影等于既有生命周期控制器的执行；复合装配展开成同一组通用操作后保持结果；与原实例无关的操作保持该实例的状态和输出。代码替换保留观测的条件是依赖规格、所选绑定和回调边界事件相同。提供或撤回原实例实际依赖的服务会改变它的纪元，并按生命周期规则触发重载。

## 可撤销效应与时间组合

论文 §3.1 的上下文状态是运行时内部可恢复的状态。外部通知、网络对端和任意业务 I/O 的撤销由相应业务契约提供。Lean 中 `RevertibleEffects.Effect α` 的对象属于 `α → α × (α → α)`：输入旧状态，返回新状态与本次取得的恢复函数。`Strict` 要求该恢复函数作用于本次新状态后得到旧状态。

`strict_composition` 证明严格效应的顺序组合仍然严格。`completed_prefix_recovers` 和 `lifo_execution_recovers` 把该结论推广到任意已完成的有限前缀；每一步记录当次实际返回的逆函数，回收按相反顺序执行。`claimed_cleanup_recovers_callbacks` 再接上现有 `Effects.claimed_steps_are_lifo`，把实际清理栈模型的顺序提升为状态恢复结论。

Rust 使用 `EffectScope::defer`、`defer_async` 保存 `FnOnce` 清理器。取消先关闭效应的 armed 标记，在途执行结束后由唯一清理者取得清理栈；`Effect::clean_steps` 按 `Vec::pop` 顺序运行清理。`Effects` 证明清理至多开始一次，`Execution` 证明挂起、恢复、错误与异步步骤保持任务归属和剩余顺序。完成性定理要求执行器最终派发工作且当前清理步骤最终返回。

论文 Definition 21 的私有清理句柄由 `PluginPrinciple.createGuard` 内部生成。`GuardArena` 对应 SlotMap 新鲜键契约的抽象分配历史，证明新位置为空、已有清理状态保持、句柄克隆指向同一状态，以及分配与清理交错后仍保持新鲜性。`private_guard_cleanup_starts_at_most_once` 将这一步连接到任意命令迹的至多一次清理。

### 论文公式的适用域

论文 Definition 2 的 `track` 使用全局逆函数。Definition 6 对单次效应只要求“本次新状态可以恢复成本次旧状态”，这允许恢复函数捕获旧值并忽略输入。Definition 7 再对恢复函数调用 `track` 时，需要补充该函数可逆的条件。

`RevertibleEffects.IsoEffect` 明确保留这一条件，并证明原文 Theorems 8、11。一般 Rust 回调采用 `Journal` 与 `lift`，保存旧清理日志和本次逆函数，证明相同的严格性及组合规律。`strict_inverse_need_not_be_injective` 给出 Bool 覆写操作的机器检查反例，说明单次恢复与全局双射分别适用于哪些对象。

论文 Algorithm 1 第 6 行在其“右侧函数先执行”的组合约定下会累计出正序清理。实际应将新逆函数放在已有逆函数的内侧，使最后取得的资源最先归还。`fifo_recovery_counterexample` 使用“先加二，再乘二”区分两种顺序：从 1 执行到 6，逆序清理恢复 1，正序清理得到 2。Rust 现有 `push`/`pop` 路径采用前一种结果；集成回归对实际清理闭包验证恢复值。

跨组件单独撤销还需要资源分离或可交换条件。`independent_component_removal` 显式使用逆函数与另一个操作可交换的前提；`separated_removal_preserves_other` 给出两个组件占用不同状态分量的具体充分条件。同一服务的依赖方通过已提交绑定保留提供方，在依赖方清理结束后归还该引用。

## 反应式依赖与空间组合

`ReactiveCoeffects.Store V` 是按服务键索引的可选值表，每个键 `k` 的值属于自己的类型 `V k`。`FiniteStore` 保存有限支撑见证；依赖规格使用有限列表，与 Rust `Inject` 的表示一致。`satisfaction_iff` 证明可执行满足性检查等价于每个所需键均存在。`notify_activating_iff`、`notify_deactivating_iff` 和 `notify_neutral_iff` 分别刻画依赖满足状态的三种变化。

Rust 还支持可选依赖和可用性谓词。它们由 `Bindings`、`KernelSemantics` 的既有模型描述：可选缺席保留零身份，必需依赖缺席时目标纪元为空；回调在锁外执行，安装结果前重新核验绑定身份、通知修订、活跃状态、谓词和值快照。`PluginPrinciple.resolve` 直接复用这一实际纪元模型，保留 Rust 的扩展语义。

论文 §3.3.3 的依赖值相等在本项目中对应已解析的绑定身份与加载修订。`Provision::set` 更新既有绑定的服务值，保留该绑定身份；重新读取可取得新值，实例纪元按现行服务契约保持。撤回并重新提供服务会取得新绑定身份。`ReactiveCoeffects.SameEpoch` 用于源码映射时取这一身份观察域。

隔离视图把一个键解析到指定 Realm；共享服务表按键与 Realm 共同索引。`isolation_keeps_other_keys`、`isolation_preserves_store` 和 `derived_isolation_recovers_parent` 证明改变一个派生视图时，其余键、共享存储和原父视图的关系。服务回收保存注册时的 Realm，`captured_revoke_ignores_later_redirection` 证明后续视图改向仍归还原登记位置。

论文 Definition 17 的当前视图公式在后续重定向后直接应用时，会选择新的 Realm；`current_realm_disposer_counterexample` 展示两个已有域上的差异。Rust `ImplData.realm` 保存取得时的域，`withdraw` 依据该字段回收。对原地覆盖视图，形式化逆函数捕获旧映射；实际 `Context::derive` 使用不可变父链，父视图继续保留原映射。

拦截元数据按声明值、外层视图、内层视图的顺序合并。`intercept_preserves_providers` 证明拦截保持服务提供表；`intercept_same_key` 在元数据结合律成立时证明连续拦截的组合规律。`overrideAlgebra` 对右侧覆盖的字段操作直接证明结合律、单位元及最近值优先。Rust 的自定义 `Merge` 由实现者提供具体代数；任意自定义合并只取得源码保证的求值顺序，结合与重分组定理使用相应代数前提。

## 统一上下文与新鲜身份

论文 Definition 24 用递归方程表达“上下文自身也包含效应和依赖”。其中 `Γ → Γ` 在参数位置出现，直接作为归纳数据类型会违反严格正性。`PluginPrinciple` 使用任意有限深度的上下文层级表示该结构，每层包含当前状态、恢复函数和依赖视图，复用 `RevertibleEffects` 的恢复及组合定理。实际 Rust 句柄通过有限 `Scope` 父链、Fiber 身份及内核表关联这些对象；`Scopes` 证明完整遍历与缓存的表示关系。

`Context::derive` 共享内核，附加不可变视图层。子组件取得新 Fiber 边界，父挂载效应负责子组件寿命。派生视图、组件嵌套与普通插件使用同一上下文机制，运行时的安全性观察落实到准入、已提交依赖和效应所有者。

Realm 和闭包组件定义的身份由 `identity::allocate` 分配。它以原子 `fetch_update` 执行检查过的递增，零为保留值，最大值为耗尽前沿；分配失败保持前沿。`Realm::new` 与 `ComponentDef::validated` 在耗尽时终止本次构造。对应证明对任意成功分配迹给出严格递增和无重复结论；真实边界执行与并发回归检查具体整数表示。

## 论文条款与源码映射

| 论文条款 | Lean 对象或结论 | Rust 入口与验证 |
|---|---|---|
| Definitions 1–2；Theorems 3–4，页 9–10 | `EffectContext`、`track_projection`、`track_composition`、`track_injective` | 效应记录的代数规格；实际回调使用下列单次恢复解释 |
| Definition 5，页 10 | `recover_idempotent`、`tracking_recovers_initial_state` | `Effect::dispose`、`clean_steps`，由一次性清理模型连接 |
| Definitions 6–7；Theorem 8，页 11–12 | `Strict`、`iso_lift_is_strict`、`operational_lift_is_strict` | `EffectScope::defer`/`defer_async`；恢复回调的业务前提 |
| Definition 9；Theorems 10–11，页 12 | `strict_composition`、`iso_lift_composition`、`operational_lift_composition` | 逆序清理及实际非交换回调回归 |
| Definitions 12–13，页 13 | `FiniteStore`、`get_returns_registered_value`、`provision_inverse_restores` | `provide_value`、`get_value`、`withdraw` |
| Definitions 14–15，页 14 | `satisfaction_iff`、三种 `notify_*_iff` | `epoch_ids`、`notify`、`refresh_keys`；Rust 可选依赖见 `Bindings` |
| Definitions 16–17，页 15 | `IsolatedContext`、派生隔离与捕获域回收定理 | `isolate_named`、`resolve_realm`、`ImplData.realm`、`withdraw` |
| Definitions 18–19，页 15–16 | `MetadataAlgebra`、`intercept_same_key`、`context_scalar_takes_priority` | `merge_meta`、`intercept_meta`、`Scoped::meta` |
| Definition 20，页 16 | `PluginPrinciple.Definition`、`resolve`；`Lifecycle` | `ComponentDef`、`Inject`、`fiber::drain` |
| Definition 21，页 17 | `Effects.cleanup_starts_at_most_once`、唯一清理申请者 | `EffectData.armed`、`running`、`cleaning` 与 `start_cleanup` |
| Definitions 22–23，页 17–18 | `completed_prefix_recovers`、`claimed_cleanup_recovers_callbacks`；`Execution` | 完成边界上的登记回调、取消等待、异步清理 |
| 纪元与异步惯性，页 19–20 | `Bindings`、`Lifecycle.trace_equiv`、`AsyncSemantics` | `target`/`running`、`inertia`、`finish_transition` |
| Definitions 24–25，页 20–21 | 有限上下文层级、严格效应提升、`Scopes` | `Context::derive`、父 Scope 与 Fiber 边界、挂载效应 |
| Algorithms 1–2，页 23–25 | LIFO 恢复、捕获域撤回；论文公式的条件见正文 | `effect.rs`、`reflect.rs`，源码探针与集成回归 |
| Algorithms 3–6，页 25–28 | `Bindings`、`KernelSemantics`、`Lifecycle`、`UpstreamSimulation` | 声明消费者索引、挂载、调和、声明访问及其上游差分 |
| Definition 26；§4.2.1，页 28–29 | 嵌套组合展开、`Deployment`、`BindingGraph` | Rust 配置与 Manager 装配；实例选择和依赖闭包 |
| Algorithms 7–9；§4.2.2，页 29–31 | Rust 对应域为 `Deployment`、`StorageProgram`、`Composition` | 本项目使用原生/脚本部署和持久化事务；JS 模块缓存属于论文参考实现 |
| §5.1–5.2，页 32–34 | `Routing`、`Protocol`、`Architecture`、`Composition` | 版本服务、跨进程 RPC、可信调用来源与资源转移 |
| §5.3–5.5，页 34–36 | 显式宿主契约、类型键、版本兼容与源码边界 | Rust trait、Future、动态库、UP/1 与版本化接口 |

论文 §4.3 的 Koishi 案例、§6 的相关工作和 §7 的结论提供经验及研究背景。其生态规模、生产效果和性能陈述保留为论文来源事实。当前验证针对本仓库的组件行为、代数规律和实现对应关系。

## 验证入口

`formal/bridges/context-paradigm.json` 记录三个新增模型的源码、入口、命题和前提。`tools/plugin_principle_probe.py` 编译实际组件定义、通用装配及身份分配片段，检查受限源码结构与完整字段来源，并执行命名反例。`crates/cordis/tests/plugin_principle.rs` 在真实内核上验证插件替换、依赖等待、隔离和清理。

```sh
python3 tools/verify_semantics.py --evidence target/formal-semantics.json
python3 -m unittest discover -s tools/tests -p test_verify_semantics.py -v
cargo test --locked -p cordis --lib --tests
cargo test --locked -p cordis-supervisor --lib --test manager
cargo test --locked -p cordis-web-deployment --lib --tests
```

统一验证器进行清洁 Lean 构建，逐项审计全部逻辑声明，执行生产源码桥接，并比较运行前后的源码摘要。生成的收据区分任意参数证明、有限执行观察和真实集成回归；具体执行结果见[上下文范式验证记录](measurements/context-paradigm.md)。运行环境、回调正确性与元数据代数前提保留在各模型及桥接条款中。
