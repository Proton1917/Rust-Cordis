# 生命周期决策与效应所有权的形式化验证

内核依据实例存活性、依赖绑定纪元、迁移进度和效应所有权决定下一步。`formal/Lifecycle.lean` 给出这部分决策的参考语义、可执行决策树及等价证明；`formal/Effects.lean` 给出取消、执行完成与清理所有权的协议和不变量；`formal/Admission.lean` 给出绑定到捕获纪元的效应准入条件。实际 Rust 生命周期路径调用由这些决策树生成的 `crates/cordis/src/lifecycle_generated.rs`。

证明的基线为 `7e4dd5e41445be82af10def53fae315b5b83f7ea` 的 `fiber.rs` 和 `effect.rs`。对应的产品语义来自[行为规格](01-semantics.md)中的 T-LIF、S-FBR、T-EFF、S-EFF；[上游一致性矩阵](04-conformance.md)继续验证真实实例、服务、事件及异步执行。

## 证明域和事实所有者

`Kernel` 保持实例状态、目标纪元、运行纪元、排队标记、通知计数和效应集合的唯一所有权。`lifecycle` 模块接收锁内读取的事实，返回纯决策，既不保存实例表，也不启动任务。调和队列、提交绑定、状态通知和宿主任务仍由各自原有入口执行。

`Fiber::is_current_epoch(&Epoch)` 在同一把内核锁内同时检查存活性、目标纪元与运行纪元相同、目标纪元与调用方捕获的纪元相同。调用方借用已有纪元，省去组合 `Context::is_current()` 与 `Fiber::epoch()` 所需的第二次锁读取和依赖向量克隆；这也是监管器判定某一代实例是否仍有效的统一查询入口。

本证明完整量化**生命周期决策接口的全部输入分类**，并证明所定义状态机在任意输入序列上的最终状态和输出序列相同。它为本次纯决策提取建立精化关系。Rust 的锁操作、状态字段适配、回调执行、服务解析和外部系统具有独立的实现义务；这些义务的当前证据是源码审查与实际回归。完整框架、原生宿主、协议和 TypeScript 上游实现之间的整体形式化等价仍需要进一步建立这些实现的操作语义与精化证明。

## 对象、输入与观察

Lean 的 `Epoch` 是 `Option (Nat × List Nat)`。`none` 表示非活跃目标；有值时依次包含修订号和按声明顺序排列的绑定身份。证明允许任意长度的身份向量和任意自然数标识。Rust 使用的 `u64` 身份是其中的子集，本次决策只读取相等关系和是否非活跃。

`State` 包含六种公开状态 `Pending`、`Loading`、`Active`、`Failed`、`Disposed`、`Unloading`，以及 `alive`、`failed`、`target`、`running`、`inertia`、`suspended`、`queued`、`notifications` 和 `effects`。后两项是任意自然数。错误的载荷、配置内容、回调函数和绑定对象保留在各自实现中，模型只使用决定迁移的事实。

| 输入 | 锁内状态变化 | 实际入口 |
|---|---|---|
| `refresh(resolved)` | 根据存活性和失败状态选择目标，撤下排队标记，决定保留、空闲、加载或卸载 | `fiber::drain` |
| `finish(loading)` | 比较目标和运行纪元，处理卸载期间新增效应，串联迁移或进入稳定状态 | `fiber::finish_transition` |
| `fail(attempt)` | 加载仍对应当前目标时记录失败并撤回目标；过时失败保留新目标 | `fiber::transition_loop` |
| `dispose` | 撤回实例存活性并请求调和 | `Fiber::begin_dispose` |
| `recover` | 清除加载错误并请求调和 | 需要重启的配置恢复、组件替换 |
| `suspend`、`resume` | 保存派发受阻的清理或恢复同一清理，改变迁移状态 | `submit_cleanup`、`resume_cleanup` |
| `notified` | 记录一次状态通知完成 | `status_changed` |
| `effects(count)` | 提供当前效应集合的基数 | 效应登记、转移与清理完成 |

`Output` 记录锁边界产生的迁移动作、状态变化对和待提交的纪元。`run` 同时返回最终 `State` 和逐步 `Output`。状态通知调用的用户代码发生在锁外，可以在随后输入中再次改变目标。证明覆盖这些输入的任意交错次序，不额外假定它们按照某个测试时钟到达。

`notified` 与 `effects` 是环境事实入口。实际代码只在存在对应通知或效应时产生这些事实。模型在更大的输入域上证明两种决策程序相等；输入可达性与真实调度的对应关系仍由适配层义务约束。

## 证明结构

`idleRef`、`targetRef`、`refreshRef`、`finishRef` 保留基线分支的优先级。对应的 `Tree` 程序采用显式字段读取和分支节点，独立定义执行形式。Lean 内核检查两者对所有布尔输入均相等；其中 `refresh` 有 128 种、`finish` 有 32 种输入，其他决策补齐剩余输入分类。

`step` 把决策结果应用于模型状态。参考程序与生成程序共享保持不变的状态字段更新，`step_equiv` 证明单步结果相同。`trace_equiv` 对输入列表归纳，保留最终状态与每个输出；`stream_prefix_equiv` 证明无限输入流的任意有限观察前缀相同。这里没有探索深度、实例数量或纪元向量长度的上界。

| 定理 | 语义结果 | 对应条款 |
|---|---|---|
| `decisions_equiv`、`step_equiv`、`trace_equiv` | 参考决策与生成决策在全部输入及任意有限迹上保持状态和输出 | T-LIF-3～7、S-FBR-1、6、8、14 |
| `stream_prefix_equiv` | 每个无限执行的有限观察前缀保持相同 | T-LIF-6 |
| `failed_refresh_preserves_target` | 依赖刷新保持已失败实例的目标 | S-FBR-7 |
| `stale_failure_preserves_state` | 过时加载的错误保留当前目标 | S-FBR-11 |
| `inertia_preserves_transition`、`suspended_preserves_transition` | 在途或已保留的清理继续拥有迁移 | T-LIF-6、派发恢复契约 |
| `active_epoch_change_unloads` | 活跃纪元之间的变化先卸载 | S-FBR-8 |
| `current_epoch_equiv` | 同一快照中的存活性、运行目标与捕获纪元联合判定 | T-LIF-7、实例换代所有权 |
| `Admission.successful_registration`、`stale_guard_denies_registration` | 登记使用当前所有者事实，满足全部代际守卫；任一守卫过期即拒绝且保留原有归属 | T-CTX-1、T-LIF-7、效应准入 |
| `Effects.cleanup_starts_at_most_once` | 任意次取消、完成与再次释放只产生一个清理所有者 | T-EFF-2、S-EFF-4 |
| `Effects.cancel_waits_for_execution`、`final_defer_retained` | 取消请求保留在途步骤及最后交回的逆变换 | T-EFF-4、5、S-EFF-5 |
| `Effects.complete_rollback` | 单个效应的已取得逆变换按后进先出顺序交给清理执行 | T-EFF-1、3、S-EFF-3 |

效应证明以线性化后的命令序列表示并发请求。实际 `start_cleanup` 在同一把内核锁内检查并取得清理权；它调用生成的 `cleanup` 守卫。模型证明此守卫结合 `running`、`cleaning` 和清理栈转移保持唯一所有者。具体回调的完成、公平调度、异常捕获、资源引用计数及父子效应环检查由实现回归另行检验。

`complete_rollback` 描述清理操作序列，假定各抽象操作完成后继续下一步。它不提供对任意插件回调终止性的证明。多个顶层效应的并发清理继续遵守 S-FBR-12、S-FBR-13。

准入模型直接复用 `currentEpoch`。`Admission.register` 在一次状态转换中检查所有者存活性和全部捕获纪元守卫，再把新效应登记给该所有者。`ordinary_context_preserves_alive_contract` 证明没有附加守卫的普通上下文保留原有存活性契约。运行时实现需在取得同一把状态锁后检查与登记，并从正确的上下文层级收集守卫；这两项是对应实现的精化义务。先在锁外查询、随后在另一个状态下登记的路径不满足该模型。

## 生成和实际编译的连接

`Generate.lean` 从已证明的树逐节点输出 Rust `if` 分支和枚举叶子，随后用仓库固定 Rust 工具链的 `rustfmt` 排版。生成文件被正常编译进 `cordis`。验证命令逐字比较重新生成的结果与仓库文件，手工改动生成文件会使检查失败。

`formal/decision_probe.rs` 直接编译生产 `lifecycle.rs`，遍历全部有限输入分类。预期值由 Lean 中独立书写的基线函数产生。这项穷尽执行检查连接了决策树打印器、Rust 编译器和生产模块；无界状态与轨迹结论由 Lean 证明提供。两层证据分别记录。

纪元相等可能扫描整条绑定向量。生成的 `refresh`、`finish` 和 `current` 接收静态 `FnOnce` 谓词，只在分支确实读取相等关系时执行。`Tree.reads` 的定理保证在途刷新、挂起刷新、卸载结束和失活查询保持短路；Rust 探针对这些入口记录谓词的实际执行次数。该提取保持原有的纪元扫描条件，新增堆分配为零。

新增联合纪元查询的八种输入也包含在编译核对中，总共覆盖 204 种决策输入。`current_epoch.rs` 的真实实例回放验证在途旧代、新目标、完成和释放时的判断，并逐项扰动绑定身份；`event_hot_paths.rs` 用同一非空依赖向量分别执行 1,000 次旧组合查询和新借用查询，记录实际分配次数并要求新入口为零。

验证还执行两个负控：让 Lean 证明一个错误的加载决策，以及把生成 Rust 的一个加载叶子改为卸载。两者都必须被相应验证环节拒绝。

## 实现精化义务

| 边界 | 当前连接与证据 | 进一步形式化所需内容 |
|---|---|---|
| `FiberData` 到决策输入 | 所有字段在原有内核锁内读取；全部决策输入穷尽核对；核心回归覆盖状态序列、通知重入、失败和并发更新 | Rust 字段读取、枚举与纪元比较的操作语义及适配映射证明 |
| 决策结果到状态字段 | 保留 `drain`、`finish_transition` 的更新次序、绑定提交和锁外通知；原有真实生命周期用例执行 | Rust 赋值、锁线性化点及 `publish` 模型更新的一致性证明 |
| 依赖解析与提交 | 核心拥有 `epoch_ids`、`commit_locked`、已提交绑定和消费者集合；服务替换与逐绑定清理回归 | 服务存储、隔离域、可用性谓词及承诺绑定的精化关系 |
| 效应回调与执行器 | 清理守卫进入生产路径；同步、异步、取消、失败及宿主拒收场景由真实执行器运行 | Future 轮询、异常、任务归还与模型输入产生规则 |
| 捕获纪元的效应准入 | 联合谓词已进入生产查询；`Admission` 完整检查传入守卫集合和所有权登记结果 | 上下文守卫收集、检查与登记共享锁的线性化、子实例交接的 Rust 精化 |
| 上游、协议及部署 | 177 行矩阵由实际执行证据绑定；协议、SDK、持久化保持各自回归 | TypeScript 与 Rust 的语言级语义映射，以及跨进程、存储恢复模型 |

证明信任基础包括 Lean 4.19.0 的内核及其标准逻辑公理、决策树的 Rust 打印器、固定 Rust 工具链和执行探针的环境。验证以 `--trust=0` 重新类型检查导入模块，逐项输出所有定理的公理依赖，只允许 Lean 的标准 `propext`、`Quot.sound`、`Classical.choice`；拒绝新增公理、未完成证明和绕过内核的求值证明。

## 复现与持续验证

需要 Python 3.11 或更新版本、仓库 `rust-toolchain.toml` 指定的 Rust 1.96.0，以及 `formal/lean-toolchain` 固定的 Lean 4.19.0。Lean 项目仅依赖该工具链随附的 `Std`，没有外部证明库。

```sh
python3 tools/verify_semantics.py --evidence target/formal-semantics.json
cargo test -p cordis --lib --tests
python3 tools/conformance_run.py --output target/conformance-report.json
```

`--lake`、`--rustc`、`--rustfmt` 可指定工具的实际路径。修改已证明决策树后，使用 `--write-generated` 同时生成生产模块并执行完整检查。验证会重新构建证明、审核全部定理、核对生成文件、编译并遍历全部决策输入，再执行两个负控；任一步缺少工具、失败或输入域不完整均返回非零退出码。

`formal-semantics.json` 记录实际工具版本、定理名单、公理依赖、编译核对结果、负控结果、耗时和源码摘要。验证前后核对证明、生成器与生产适配文件的摘要；`--write-generated` 只允许本次预期的生成文件变化。每次开始会撤销旧的通过状态，失败时写入失败阶段和错误，避免读取者误用上次结果。摘要用于绑定证据到文件内容。证明、有限输入核对和真实异步回归分别承担各自的接受条件。

组件一致性工作流按上述顺序检查证明、内核基础与并发回归、联合纪元查询、全部适用上游场景，并保存两份独立证据。Lean 二进制版本和下载摘要固定，Rust 版本沿用仓库工具链文件。
