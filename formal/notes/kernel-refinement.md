# 内核表示关系与服务绑定精化

`KernelRefinement.lean` 将有限宽纪元、`FiberData` 字段、持久作用域和锁内效应登记映射到现有生命周期与准入语义。`Bindings.lean` 给出依赖身份解析、隔离域查找、已提交绑定、快照重验和消费者保留的逻辑规格。两组定理由 Lean 4.19.0 内核检查。

生产代码连接由 `check_kernel_refinement.py` 执行：从当前 Rust 文件唯一定位并提取函数体或锁内代码段，将原文放入独立探针，编译执行，并与 `KernelVectors.lean` 的可执行结果逐行比较。Lean 定理覆盖任意长度的对应数学对象；编译探针提供有限输入上的源码连接证据。Rust 语言语义、编译器和容器实现处于本模型的可信边界。

## 表示关系

| 生产对象 | Lean 表示与映射 | 核心性质 |
|---|---|---|
| `Epoch::Inactive` | `Epoch64.inactive` 投影为 `none` | 活跃性判断保持 |
| `Epoch::Active { revision, impls }` | `UInt64` 修订号与有序 `List UInt64`，逐项投影到 `Nat` | 投影单射，完整纪元相等判断保持 |
| `FiberData` 的状态字段 | `FiberData` 保留错误、任务的 `Option`、`USize` 通知计数和效应身份列表 | 错误与任务投影为存在性，效应列表投影为长度 |
| `Scope` 父链 | `View` 保留每个纤程边界的 owner 身份及父指针 | 对合法构造链，按 owner 变化截断等价于按 Fiber/Root 层截断 |
| `State` 中的效应登记 | 所选 owner 的 `FiberData` 与 `(effect, owner)` 记录列表 | 同一锁内快照决定准入，两张表一起更新 |
| 依赖条目与绑定槽 | 有序 `Dependency` 列表、`Option` 身份列表与实现表 | 长度一致、必需依赖存在、可选缺失为零、顺序保持 |
| 服务作用域 | 从使用点到祖先的 `Frame` 列表及隔离层列表 | 本地供给优先、最近隔离域有效、声明缺失截断祖先回退 |
| `Arc` 快照身份 | 用保持相等性的身份表示 predicate/value 指针 | 提交前逐项重验；没有捕获 value 时保留原来的跳过检查语义 |
| `dependents` 集合 | 绑定身份到消费者集合的成员关系 | 外部已提交消费者阻止提供方完成等待；释放只撤回该消费者 |

Scope 的 `Constructed` 关系包含实际构造条件：普通派生和纪元绑定继承 owner；子纤程分配的 owner 与当前父 owner 不同。`owner_boundary_matches_layer_boundary` 通过构造归纳证明整个有限父链的扫描等价。`scope_admission_refines` 再把扫描结果、缓存位、owner 存活性和当前纪元检查组合为已有 `Scopes.fast` 语义。

## 锁内生命周期模拟

`refresh_boundary_simulation` 证明下面两种计算得到相同的抽象状态和输出：先运行具体字段更新再投影；先投影再运行 `Lifecycle.step generated` 的 refresh 分支。证明同时处理目标选择、旧目标比较、队列标志清除、加载或卸载状态、运行纪元、惯性和通知计数。加载输出携带本次目标，供 `commit_locked` 固定对应绑定。

`finish_boundary_simulation` 对一次 `finish_transition` 锁内迭代给出相同关系。其入口条件包括 `inertia = true`：生产实现继续已有转换时保留惯性，refresh 的启动分支设置惯性。完整运行时必须保证进入转换迭代时满足这个条件。`finish_never_keeps` 证明生成的 finish 决策不会返回 Keep，因而 Rust 将 Keep/Idle 合为无下一转换的分支与本模型一致。

通知自增的条件为 `notifications.toNat + 1 < USize.size`。这一条件使有限宽加法与自然数加法一致；通知自减的条件为 `0 < notifications.toNat`。修订号的对应条件使用 `UInt64.size`。这些是数学表示关系的运行前提，检查脚本同时执行零计数和 `usize::MAX - 1` 附近的生产更新。跨越整数回绕、分配失败或无效表身份的行为由运行时实现及其独立测试负责。

`locked_registration_simulation` 证明具体准入和 owner 效应列表更新精化 `Admission.register`。`successful_registration_uses_same_snapshot` 给出成功登记对应的准入事实及两个表中的新增记录；`successful_registration_records_view_owner` 在所选记录的 owner 与当前视图 owner 一致时建立视图所有权。`fresh_registration_preserves_unique_effect_ids` 把分配器提供的新鲜身份条件传递到 owner 列表的无重复性。生产桥接直接执行 `Context::create_effect` 的原函数体，其中锁在准入检查与两张表插入之间一直保持。

## 依赖与服务绑定

`epoch_resolution_spec` 是依赖解析的双向规格。成功结果当且仅当绑定槽数与声明数相同，并且结果中的每个位置满足 `SlotMeaning`：该槽查到了相同的实现 UID，或者该依赖可选、实现缺失且该位置为零。`collect_meaning` 对任意输入列表归纳证明这个关系；`resolved_epoch_preserves_length` 给出结果长度保持。`required_slot_has_binding` 从必需依赖位置的规格中提取真实实现表记录。零作为缺失标记的唯一性通过 `zero_identifies_optional_absence` 表达，其前提是有效实现 UID 为非零值。

`committed_membership` 刻画 `commit_locked` 的 zip/filter-map 结果，包括列表长度不一致时的 zip 截断行为。`latest_matching_provision_wins` 证明最近登记的同键同域本地供给优先。`isolation_seals_ancestors` 和 `declared_absence_seals_ancestors` 刻画隔离屏障与可选依赖缺失的作用。`realmFor` 证明最近匹配隔离层决定域，其余键的隔离层保持当前服务的域。

`committed_read_survives_live_replacement` 证明组件已提交绑定在 live store 替换后保持相同身份。根上下文读取 live store 的分支保留在 `getBinding` 中。此定理刻画查找的身份选择；`get_value` 的动态类型转换、错误对象和元数据合并继续由生产集成测试验证。

`snapshot_validation_spec` 给出刷新快照可提交的完整条件：依赖索引仍对应原服务键，当前 binding、通知修订号、活跃状态和 predicate 身份均匹配，已捕获的 value 身份也匹配。`stale_notification_cannot_commit` 证明通知版本变化会保留此前绑定槽。`incremented_notification_invalidates_snapshot` 将有限宽自增的表示条件与这一提交规则连接。提供方检查和消费者 predicate 的运行结果作为 `available` 输入进入模型；模型保留回调在锁外执行、返回锁内重验后写入的边界。

消费者集合使用任意身份上的成员关系。`retained_membership_spec` 与 `released_membership_spec` 连接可执行成员位与集合规格；`provider_waits_for_committed_consumer` 证明提供方仍有外部已提交消费者时等待条件尚未成立。释放规则保留其他消费者，并删除本次 committed 列表中的当前消费者。源码桥接运行完整 `wait_dependents`、`release_committed` 函数体，以真实 Waker 和显式轮询观察等待与释放的结果。

## 生产源码连接

| 源码文件 | 实际提取并编译的内容 |
|---|---|
| `crates/cordis/src/fiber.rs` | `Epoch` 类型、`FiberData::idle_state`、`FiberData::is_current`、`drain` 锁内更新段、`finish_transition` 锁内更新段 |
| `crates/cordis/src/context.rs` | `Context::derive`、`Context::permits_effect` |
| `crates/cordis/src/effect.rs` | `Context::create_effect` |
| `crates/cordis/src/reflect.rs` | `epoch_ids`、`commit_locked`、`locate`、`scope_realm`、`wait_dependents`、`release_committed`、`refresh_keys` 的完整重验写入循环 |
| `crates/cordis/src/lifecycle.rs` 与生成模块 | 由探针直接引入，执行生产决策函数 |

每个提取项在证据 JSON 中记录入口、源路径、字节数和 SHA-256。缺失、重复或顺序不符合要求的锚点会使提取失败。验证前后比较输入文件摘要，保证向量、证明模型、探针和生产源码属于同一快照。

探针的支持层将 SlotMap 身份表示为有效且稳定的 `usize` 索引，把所观察的 `DisposableList` 接口表示为有序 Vec，并用 `std::sync::Mutex` 提供锁。异步提供方等待使用 `std::future::poll_fn` 和显式单次轮询；生命周期任务、回调和错误负载采用只保留存在性的值。快照重验使用实际 `Arc::ptr_eq`。这些适配器、SlotMap 的分配和代次复用、停车锁实现、执行器调度、内存安全、宿主回调行为以及唤醒后的执行公平性，均列在 JSON 的 `support_stubs` 中。

## 验证入口与观察范围

```sh
cd formal
lake build KernelRefinement Bindings
cd ..
python3 formal/check_kernel_refinement.py \
  --lake lake --rustc rustc --evidence /tmp/cordis-kernel-refinement.json
```

检查器先构建当前 `KernelRefinement` 与 `Bindings` 源码，再执行向量模块，并核对每族的输入域、基数及唯一性。当前向量生成器产生 59,990 个独立输入：

| 操作 | 输入数 | 主要变化维度 |
|---|---:|---|
| refresh 锁内更新 | 24,576 | 六种状态、存活/错误/惯性/暂停/队列/效应标志、目标与运行纪元、解析纪元 |
| finish 锁内更新 | 12,288 | 六种状态、两种转换方向、相同或不同纪元、效应与错误标志 |
| 效应登记 | 7,680 | 存活性、已拥有记录、纪元匹配、六种视图构造、深度 0/1/2/17/129 |
| 依赖身份解析 | 5,362 | 长度 0 至 4、必需/可选、存在/缺失/失效身份、绑定槽比声明长或短一位 |
| committed 固定 | 484 | 可选槽、重复使用绑定、提供方与消费者是否相同 |
| 隔离与绑定查找 | 1,024 | 本地供给优先、较新供给、隔离屏障、祖先、已提交和声明缺失 |
| 快照重验 | 256 | binding、revision、active、predicate、value、服务键及 available |
| 消费者释放与等待 | 7,744 | committed 列表、两个绑定、当前与其他消费者、提供方资源集合 |
| 通知计数边界 | 576 | 零值、最大值前一位及状态是否变化 |

源码负控制反转 `Context::permits_effect` 存活分支，并将 `epoch_ids` 的双向长度不等检查改为单向小于检查。前一项在 2,680 个登记输入上检出变化，后一项由绑定槽长于声明的向量检出。检查器还拒绝向量缺项、重复和越域。正常源码与 Lean 的观察结果逐项相同，证据分别记录负控制拒绝状态和具体见证。证据文件通过原子替换从 running 进入 passed 或 failed，失败重跑会撤销旧的成功状态。
