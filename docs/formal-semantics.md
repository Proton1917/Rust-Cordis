# 运行时语义的形式化验证

形式化工程覆盖生命周期决策、内核字段与上下文表示、依赖图构造、原子与异步执行、资源所有权、CBOR 与协议请求、持久化程序及恢复、上游状态模拟，以及组件事实来源和跨模块组合。Lean 4.19.0 检查状态模型与表示精化的定理；受限源码解析器为实际表达式和控制流生成任意参数证书；原样源码探针及 Rust、TypeScript、Python 回归检查具体执行。各类证据分别记录，并绑定到当前源码摘要。

`formal/` 包含 28 个证明模块。生命周期生产路径使用 `formal/Generate.lean` 生成的 `crates/cordis/src/lifecycle_generated.rs`。其余模块给出具体表示的投影、转移关系和所有权不变量，各模块的源文件、入口、定理、前提与回归命令列在 `formal/bridges/*.json`。`tools/verify_semantics.py` 检查模块清单、桥接清单、全部 Lean 声明、公理依赖和实际源码探针，并将结果绑定到文件内容摘要。

生命周期参考分支的来源是提交 `7e4dd5e41445be82af10def53fae315b5b83f7ea` 的 `fiber.rs` 和 `effect.rs`。上游 TypeScript 来源固定为 `261c5fdae6edb54e7ed4f071024f530acd18903b`。行为条款见[组件组合的行为规格](01-semantics.md)，实际场景映射见[上游一致性矩阵](04-conformance.md)。

## 证明对象与结论

| 模块 | 对象与主要结论 | 生产连接 |
|---|---|---|
| `Lifecycle` | 六种状态、纪元、迁移标记与输出；参考决策和决策树在全部输入、任意有限迹及无限流的有限前缀上相等 | 生成并直接编译生产决策模块，穷尽 204 个输入分类 |
| `DecisionProgram` | 参数读取的所有权；求值保留结果和读取次序，每条合法路径至多读取一次同一参数 | 生成器只接收 `Certified` 树，参数名用定长 `Vector`；Rust 探针核对 `FnOnce` 调用次数 |
| `KernelRefinement` | `UInt64`/`USize` 到数学值的投影、refresh/finish 字段更新、`FiberId` 守卫边界、同快照双表登记 | 从实际源文件提取字段适配和锁内更新片段，编译后与 Lean 向量比较 |
| `Bindings` | 任意长度依赖向量、可选缺席、提交绑定、隔离访问、通知快照校验、消费者保留与释放 | 提取 `epoch_ids`、`commit_locked`、`locate`、快照安装、依赖等待与释放入口 |
| `Effects` | 唯一清理申请者、取消后的在途步骤、最后交回的逆变换、清理栈顺序 | 生产清理守卫、异步释放及拒收恢复回归 |
| `Admission` | 存活性与全部捕获纪元守卫在一次登记中共同成立 | 同锁登记投影与生产片段 |
| `Scopes` | 任意合法不可变派生链的缓存与完整遍历相等；追加守卫保留旧约束；子纤程建立自身范围 | 构造链投影及实际 `Context::derive`、`permits_effect` |
| `Execution` | 任意任务事件轨迹保持唯一 token；拒收、悬挂和恢复保持任务归属；异步清理保持 LIFO 与首次错误 | 实际调度器接受、拒收、异常与恢复回归 |
| `NativeOwnership` | Future 存活和同步 poll 期间保留 permit；额度有界；动态库关闭条件与关闭至多一次 | native callback 测试及真实动态库退休驱动 |
| `Routing` | 每个出口引用至多归还一次；非法 release 原子拒绝；批次提交/回退、退休清扫、索引一致性 | 路由目录真实类型、析构锁外执行及各次级索引回归 |
| `Protocol` | 首次写入与取消的权利交接、单次终态记账、回复权、FIFO、字节预算和 live-lease 账本 | 实际 `Lease` 方法差分及首字节、超时、背压回归 |
| `SdkProtocol` | TypeScript 移除 pending 与 Python `future.done()` 两种终态机制的单次结算和载荷归属 | 原样提取响应方法，使用真实 `Map` 与 `asyncio.Future` 执行 |
| `Persistence` | 任意合法写入轨迹的旧/新版本崩溃原子性、确认后的持久性、可重放恢复及检查点缓存事实 | 实际存储故障注入、日志恢复与目录同步回归 |
| `Deployment` | 任意有限依赖图上的选择恰为可达闭包；最小性、无重复、终止与目录过滤 | `select_instances` 工作栈映射及实际进程管理回归 |
| `Upstream` | 共享生命周期输入域上的状态、迁移动作与过时错误归属；任意输入列表上的逐点观测相等 | 固定上游四个真实方法体及独立的一致性场景矩阵 |
| `KernelSemantics` | 控制循环入口不变量、机器身份耗尽、通知快照永久失效、完整服务读取与动态类型结果 | 实际内核入口、`Any` 下转与整数边界差分，生产边界回归 |
| `BindingGraph` | 当前与提交绑定、root 服务、隔离域、接口次版本、替换提供者及父子边构造，接入可达闭包 | 完整 `select_instances` 原样编译；候选条件 AST 的任意参数证书 |
| `Concurrency` | clone 借用、过时读取、CAS、释放、退休观察和映像取出之间的任意交错 | 真实标准库原子及 `Arc`；完整 lease 方法源码探针 |
| `AsyncSemantics` | poll、wake、Pending、取消、析构、permit 归还的微步骤；公平调度下清理最终完成 | 完整 `Admitted`/`Permit` 实现和实际 Future/Waker；清理集成测试 |
| `WireSemantics` | UP/1 信封、长度前缀、分片、多帧、整数宽度、剩余截止时间和请求标识前沿 | 真实 `integer_width`/`update_budget`/allocator 源证书及编译运行 |
| `RequestSemantics` | 整段请求的响应结算、发送完成、恢复执行、交付和取消；嵌套截止时间 | 实际 Python request 和 TypeScript Connection 的异步执行及泄漏负控 |
| `CborSemantics` | 任意递归数据的 CBOR 字节语法、浮点位模式、bignum 归一化、资源标签和节点/深度预算 | 实际 `cordis-up` codec 与支撑库行为；资源检查源码证书 |
| `StorageProgram` | 逐 I/O 程序位置、错误返回、实际日志构造与原字节重放；任意前缀的恢复原子性 | 实际文件系统、逐调用失败与进程中断、恢复重试 |
| `StorageControlFlow` | 文件发布、日志确认、写入分组、提交屏障的有序程序，任意长度计划和删除项 | 当前源码的受限控制流证书与完整方法执行 |
| `DirectoryInitialization` | 子目录到祖先目录的同步顺序、失败停止和完整重试 | 原样初始化函数，实际目录与每个屏障失败测试 |
| `UpstreamSimulation` | 目标、尝试纪元、失败、忙碌阶段、清理栈和绑定提供者的状态关系；停顿步模拟 | 非空清理和提供者释放的真实上游方法执行 |
| `Architecture` | 事实权威、修订快照、版本契约、资源守恒、单调转移标识和可信来源 | 元数据过滤及来源字段的任意参数证书；完整监管器方法 |
| `Composition` | 首次写入权与延后 hook、非空路由批次、清理身份和实际日志恢复之间的组合不变量 | 跨组件源码映射与真实路由/协议联动回归 |

定理的量化范围由 Lean 声明确定。容量可以是任意自然数，依赖向量、清理栈和输入迹可以有任意有限长度，部署图可以有任意有限规模。具体机器字的增减和投影具有相应界限条件。有限差分用例用于检查源程序连接，其用例数单独记录。

## 内核状态、纪元与字段精化

数学纪元 `Epoch` 是 `Option (Nat × List Nat)`：`none` 表示非活跃目标；有值时包含修订号和按声明顺序排列的绑定身份。`KernelRefinement.Epoch64` 使用 `UInt64` 表示实际修订号和身份。投影的单射性证明两个具体纪元相等恰好对应两个数学纪元相等，保留非活跃分类及依赖向量的全部元素。

`Lifecycle.State` 保存公开状态 `Pending`、`Loading`、`Active`、`Failed`、`Disposed`、`Unloading`，以及 `alive`、`failed`、`target`、`running`、`inertia`、`suspended`、`queued`、通知数和效应数。`Output` 记录迁移动作、状态变化对和待提交纪元。状态机输入包括刷新、迁移完成、加载失败、释放、恢复、清理悬挂/恢复、通知完成和效应集合变化。

`trace_equiv` 保留最终状态与逐步输出；`stream_prefix_equiv` 保留无限输入流的任意有限观察前缀。`KernelRefinement` 分别定义实际字段更新与数学更新，证明刷新、迁移完成和通知发布的模拟关系。`KernelSemantics` 进一步从控制器轨迹推出迁移循环入口的 `inertia = true` 条件。机器计数仍具有容量边界，减法由相应通知所有者提供前提。

实际重启修订和实现身份使用检查过的递增，耗尽时返回错误且保持原状态。通知快照的修订号耗尽后永久失效，重新提供服务取得新的实现身份。证明与生产回归同时覆盖这两种策略，避免饱和计数使旧快照重新匹配。完整 `get_value` 模型覆盖缺席、隔离、已提交绑定和动态类型错误，源码桥接实际执行普通类型、字符串及 trait 对象的 `Any` 下转。

`Bindings.epoch_resolution_spec` 将依赖解析写成逐槽关系：必需依赖具有存在、可用的绑定；可选缺席使用保留的零身份；成功结果与依赖列表等长且顺序一致。提交只保存实际绑定。访问优先考虑当前纤程自身的提供项，再处理隔离边界、已提交绑定与声明缺席。通知回调返回后，安装结果需要重新校验键、绑定、修订、活跃状态和可用性快照，过时通知保持当前绑定。

具体上下文使用不可变父链和 `FiberId`。`owner_boundary_matches_layer_boundary` 证明合法构造链的身份边界与模型层边界收集相同守卫。`locked_registration_simulation` 使用同一 `LockedState` 读取全部准入事实并更新效应表与所有者表。SlotMap 的新鲜身份、`Arc` 的不可变共享和互斥锁语义构成该表示的运行环境契约。

## 决策生成与短路求值

`Tree.eval` 返回决策，`Tree.reads` 返回实际读取的参数序列。`DecisionProgram.execute` 在读取前消费该参数的调用权；第二次读取已消费参数时返回失败。`Safe` 递归要求每条路径上的参数新鲜，允许互斥分支各自使用同一参数。`execute_preserves_result_and_calls` 证明所有合法树的求值结果和读取顺序与参考树一致；`certified_calls_once` 证明读取序列无重复。

生成入口接受带有 `Safe [] tree` 证明的 `Certified`，参数名采用长度等于树输入数的 `Vector`，字段选择由 `Fin` 保证有界。打印器产生 Rust 分支与枚举叶子，经固定工具链的 `rustfmt` 排版。验证命令逐字核对生成结果，并直接编译生产 `lifecycle.rs`。

`decision_probe.rs` 穷尽 204 个分类输入，同时核对相等谓词的调用次数。预期决策来自独立书写的参考函数。纪元向量的相等比较继续按分支短路执行；当前查询和联合纪元查询借用原有向量。实际分配预算由核心回归验证。

## 任务、原生资源与路由所有权

`Execution` 用一个任务 token 表示待提交、宿主持有、正在执行、可恢复、悬挂或已完成的唯一归属。任意事件序列保持 token 守恒。宿主已经开始执行的任务保持宿主归属；可恢复的拒收分支取回原任务。取消改变停止请求，正在执行的 Future 继续持有任务及已取得资源。

清理状态分为已完成、当前步骤和待执行步骤，三段拼接始终等于最初的 LIFO 清理序列。错误保留第一次失败，剩余工作继续归属当前清理者。`claimed_cleanup_completes_registered_stack` 将 `Effects` 的栈移交定理与异步清理完成性质组合起来。

`NativeOwnership` 分别描述 callback permit、活跃用量账本和动态库 lease。Future 销毁完成后才归还 permit，同步 poll 保留额度。关闭映像需要满足退休、零活引用、描述符释放完成和允许卸载的策略；任意合法轨迹保持关闭至多一次。

实际 lease 的 `count.fetch_update` 与 `retired.load` 涉及两个原子对象。`Concurrency` 分开表示借用源租约、加载旧计数、CAS 成功或重试、归还源锚点、读取退休位、读取零和取得映像锁。计数读取允许观察修改历史中的旧值；借用锚点阻止计数归零，归零后全部克隆入口关闭，因此旧的零观察仍对应当前零。任意交错保持映像取出时没有活跃租约或在途克隆，映像至多取出一次。

`AsyncSemantics` 将轮询中的 wake 保存到通知位，Pending 根据通知重新排队；取消、Future 析构和额度归还分别发生。安全性定理允许任意事件序列。最终清理定理使用明确的活性条件：每个未结束后缀最终得到有效派发、当前回调返回或终态发布。一个自然数秩随这些有效步骤下降，从而推出原清理栈的完整 LIFO 结束。Rust 原子/指针契约、执行器实现与外部析构器作为运行环境条件记录。

`Routing` 按出口 token 追踪目录、批次、已分离待析构和已归还状态。目录移除与资源析构分成两步，对应实际锁内摘除、锁外 `drop`。退休标记与索引清扫也分成独立事件，保留其间允许的交错。目录外的 `Arc` 克隆由取得它们的调用者持有，网络发送与对端行为具有各自契约。

## 协议与 SDK 终态

`Protocol` 对单个请求分开描述首次写入权、取消通知、提交/回退 hook 和终态领取权。取消在首次写入前发生时阻止帧启动；首次正长度写入已经取得 token 后，后续取消保留原提交所有者。入站取消发送权和 `serve_request` 的回复权分别建模，取消通知保持唯一终态回复的归属。

预算模型将已分配字节分为普通额度与控制保留额。`allocation_conserves` 给出逐次增长的准确记账，live-lease 账本将用量等式连接到实际所有者；从账本推导释放减法的前置条件，并证明任意获取、增长和释放轨迹有界。配置中的 `maximum + reserve` 需要符合 `Options::validate` 的 `usize` 检查。健康计数模型证明记账次数与继承截止时间规则，源码的 `u32` 饱和算术属于其数值表示边界。

TypeScript 在响应路径先移除 pending 项，Python 通过真实 Future 的完成状态拒绝重复结算并在请求退出时清理项。`SdkProtocol` 分别保留这两种状态变化，证明单次结算、迟到载荷的唯一释放者，以及载荷已交付后的归属。`RequestSemantics` 继续区分响应写入 Future、发送结束、协程恢复、返回结果和中途取消；任意事件序列保持载荷守恒，请求退出时不存在无人持有的载荷。

Python 请求保存自己的 Future 与是否交付的标记。响应已经进入 Future、调用协程尚未返回时发生取消或发送失败，由请求的 `finally` 关闭未交付载荷。实际回归覆盖该窗口、正常交付及迟到响应。TypeScript 整段 Connection 通过真实 Promise 调度覆盖发送成功/失败与响应、超时和取消的交错。

协议预算探针提取实际 `buffer.rs` 中的 `Lease` 类型和方法，使用顺序互斥适配器编译。SDK 探针执行当前响应方法及整段请求协程，使用真实 JavaScript `Map`、Promise 和 Python `asyncio.Future`；消息校验、错误构造和载荷计数适配器逐项列在证据中。协议会话模型将 FIFO、首字节提交、回退及响应归属联合起来。嵌套截止时间取父级和本地期限的较小者，任意派生深度保持期限单调。

## 信封、帧与递归 CBOR

`WireSemantics` 给出 UP/1 信封和四字节长度前缀，证明合法输入的编码解析关系以及任意分片重新组装、多帧顺序和错误形状拒绝。实际 Rust 表达式产生整数宽度、追加整数编码和编号分配的任意参数证书。预算写入的截断与头部更新通过封闭语法检查，前缀和帧置换定理具有合法旧帧、偏移量和长度条件；完整函数另经编译执行对照。

`CborSemantics.Data` 保存空值、布尔值、正负整数、浮点位模式、字节串、UTF-8 文本、数组、映射及标签。独立字节语法描述定长编码、不定长集合和分块字符串；递归证明覆盖任意有限树和未消费的输入后缀。值策略同时检查映射键和值中的资源标签，并描述节点、集合长度和嵌套深度限制。

codec 的相等关系遵循实际表示：Rust/ciborium 浮点缩窄在扩展后保持原位模式，包括 NaN 载荷及有符号零；ciborium 对标签 2/3 的定长且最多 16 字节的字节串按大整数解释，负大整数超过其有符号范围时拒绝。递归归一化组合定理适用于生成器的定长编码输出；实际不定长字节串保留普通标签，该行为单独建模并执行验证。资源策略作用于归一化后的树，因此嵌套整数标签的合法资源与溢出传播具有明确结果。依赖库解析器、UTF-8 和浮点转换原语具有相应实现契约。

数学整数树另由 `MathValue` 和独立 `Denotes` 关系描述。去除前导零保持大小值；u128 与负 i128 叶子转成合法 scalar 或数值标签，得到规范化不动点。`recursive_integer_tree_roundtrip` 将这一步推广到任意有限数组、映射的键和值及合法普通标签，随后接上字节解析和资源策略，保留从 `i128::MIN` 到 `u128::MAX` 的逐节点整数语义。

## 持久化与部署选择

`Persistence.Image` 将路径身份映射到文件内容或缺席。写计划给出所有会变化的名字，其余名字满足 frame 条件。模型按备份日志准备、文件写入与同步、逐项改名/删除、数据目录同步、决定标记移动和决定目录同步区分阶段。决定标记移动后、目录同步前的崩溃允许保留或丢失该移动。

`crash_recovery_is_atomic` 证明任意合法转移后完成恢复得到旧映像或完整新映像。`acknowledged_deployment_survives_crash` 证明决定确认后恢复得到新映像。恢复按原日志逐项重放；`interrupted_recovery_can_restart` 证明任意部分恢复后重新开始仍得到原映像。检查点缓存只有在文件与目录事实满足时才能成为可复用事实，目录同步失败清除该缓存事实。

`StorageProgram` 为每个具体 I/O 返回点设置程序位置。备份顺序和数据写入顺序可不同：日志逐项保留原字节，日志发布前所有修改名均已覆盖，恢复直接读取该日志。任意成功/失败返回及允许的落盘事件都投影为 `Persistence` 的可达状态，最终定理因此覆盖实际构造日志的旧/新版本恢复。失败调用停止后续正常 I/O；恢复自身可以在任意重放前缀中断并重试。

`StorageControlFlow` 从受支持的源码控制流提取文件发布、备份与日志确认、BASE/状态/配置/来源写入分组、数据屏障和决定屏障的有序模板。Lean 对任意计划、备份顺序及删除项检查其执行次序与程序位置吻合。原样 Rust 方法另外在真实文件系统上执行，逐个 I/O 注入返回错误和进程中断，再执行实际恢复。

`DirectoryInitialization` 覆盖首次创建与失败后的再次启动。初始化按 `consumed`、`data`、存储根和逐级祖先的顺序确认目录内容；任何失败停止当前尝试，已存在目录也完整重试。初始化成功推出所有必需目录屏障均已成功，后续事务据此取得可达存储根的前提。

文件系统契约要求成功文件同步保留完整内容，单个名字的改名具有原子性，成功目录同步持久化相应目录项，部署锁排除外部写者。结论描述恢复成功完成后的映像；持续 I/O 错误可以使恢复继续待处理。

`Deployment` 接收已构造的依赖图快照，边从提供者或父实例指向依赖者。工作栈弹出名称；首次选中时消费其邻接项并加入后继。终止度量为待处理项数加剩余邻接表中的边条目数，每个非终结步骤使其精确减少一。迭代次数因此至多为初始根条目数加初始边条目数，允许环、自环、重复根和任意有限图大小。

`selected_iff_reachable` 证明选择结果恰为根的可达闭包，`selection_is_minimal` 证明其最小性。`BindingGraph` 构造该图的实际前置阶段，包含当前/回退提供者、替换目录、已提交绑定、接口键与次版本、继承隔离域、命名域和父实例边。绑定到 root 服务会封闭候选查找并保持无实例边；它与未绑定状态具有不同表示。候选条件的源码 AST 经任意参数证书检查，完整 `select_instances` 方法通过 32,768 个输入分类执行对照，随后接上任意规模的可达闭包定理。

## 上游映射与证据范围

`Upstream` 的刷新映射采用存活、未悬挂所有者这一共享输入域。TypeScript 的早返回与 Rust 的稳定空闲决策映射到同一个“无加载/卸载动作”观察。`UpstreamSimulation` 进一步保留目标纪元、已尝试纪元、失败归属、忙碌状态、清理队列和已提交提供者的状态关系。状态发布与执行器让步允许为停顿步；任意共享微步骤轨迹保持模拟关系，并覆盖非空逆序清理、旧回调和提供者释放。

`check_upstream.py` 检查固定提交及未修改的 `fiber.ts`，提取 `_getState`、`_setEpoch`、`_reload`、`_unload` 的实际方法体。探针覆盖旧回调开始前和进入等待后两种纪元替换窗口，检查后续错误保留新目标，并实际执行非空清理和提供者释放。方法签名由执行器提供，方法体内仅去除 `Promise<void>` 的类型参数。回调、日志和服务集合的支撑对象在证据中逐项列出。

## 架构契约与跨模块组合

`Architecture` 为每类事实指定权威写入者，发布从其当前状态取得值并检查版本契约。任意事件迹保持来源合法、修订单调和当前快照一致。资源在实例持有、请求在途、清理及归还状态间转移，单调请求前沿防止旧回调匹配新转移。逐资源守恒和互异资源的 frame 规则支持组合多个实例。

监管器的两个具体连接具有源码证书：保留键过滤保证 `cordis.` 字段全部来自受信上下文；来源字段投影保证连接、上下文、实例、组和插件取自已解析视图。完整方法源码同时覆盖弱引用退役、连接身份、当前纪元、停止及导出状态。

`Composition` 将协议首字节提交与路由 hook 的延后执行分开。准备事件实际建立非空批次；获得发送权后保持唯一提交所有者，取消、断连和回退在回调间隙中交错。回退先从目录摘除再在锁外析构。全局状态同时保持事实来源、资源守恒、协议/路由所有权、清理栈和存储可达性；清理归还还检查资源身份、所有者与完成条件。全局恢复通过存储事件投影调用实际日志重放定理。

验证范围由具体证明域、源程序连接和环境条件共同界定。Lean 内核检查上述模型、表示投影及受限源码证书。完整 Rust/JavaScript/Python 语言与编译器、提取器和依赖库实现、标准库原语、外部插件、操作系统与存储设备构成实现边界；最终清理另使用明确的派发及回调终止条件。桥接清单对每个域分别给出这些条件，证据文件的 `unproved_boundaries` 保留相同范围。

## 复现与持续验证

证明检查需要 Python 3.11 或更新版本、Lean/Lake 4.19.0、仓库固定的 Rust 1.96.0 与 `rustfmt`，源码探针还需要 Node.js。CI 使用 Node.js 24.22.3。原生跨编译器验收另需 Rust 1.85.0。Python SDK 回归依赖 `sdks/python/requirements.txt`，TypeScript SDK 回归依赖锁定的 npm workspace。

上游源码使用与一致性矩阵相同的固定版本：

```sh
git init cordis
git -C cordis fetch --depth=1 https://github.com/cordiverse/cordis.git 261c5fdae6edb54e7ed4f071024f530acd18903b
git -C cordis checkout --detach FETCH_HEAD
python3 -m pip install -r sdks/python/requirements.txt
python3 tools/verify_semantics.py --evidence target/formal-semantics.json
python3 -m unittest discover -s tools/tests -p 'test_verify_semantics.py' -v
```

验证依次检查封闭的 Lean 模块清单、桥接模块和定理归属、清洁构建、逻辑声明的公理依赖、生成文件、实际编译决策、各源码探针和负控。`Audit.lean` 遍历内核环境，覆盖私有声明及生成的安全辅助声明；`--trust=0` 重新检查导入模块。允许的逻辑公理为 Lean 标准的 `propext`、`Quot.sound`、`Classical.choice`。Lean 代码生成器另外产生带内核 `isUnsafe` 标记的编译阶段常量，其元数据和依赖单独保存与计数；内核拒绝逻辑证明依赖这类常量，对应的伪造证明负控也会实际执行。源码层继续检查显式证明逃逸入口。

源码中的新增模块、未登记导入、未完成证明、额外公理和绕过内核的证明求值会使检查失败。桥接输入域需要完整且唯一；错误 Lean 定理、被修改的生产分支、错误预算计算和 SDK 重复结算路径均作为负控运行。验证前后重新枚举文件并比较摘要，覆盖运行过程中新增文件的情况。

每次运行先原子写入 `running`，成功写入 `passed`，失败写入错误阶段与 `failed`。原有通过结果随新尝试开始而失效。独立源码探针采用同样的证据失效策略，并使用当前证明输入。`--write-generated` 只允许预期的生成文件内容变化；工具路径可用 `--lake`、`--rustc`、`--rustfmt`、`--node` 指定。

实际集成检查按 `formal/bridges/*.json` 的命令入口执行，主要包括：

```sh
cargo test -p cordis --lib --tests
cargo test -p cordis-up -p cordis-routing --lib --tests
cargo test -p cordis-host --lib
cargo test -p cordis-host --test native native_retirement_metrics_and_open_handle_policy -- --exact
cargo test -p cordis-runtime --lib storage::tests::
cargo test -p cordis-supervisor --lib --test manager -- --test-threads=2
cargo test -p cordis-web-deployment --lib --tests
npm ci --ignore-scripts --prefix sdks/typescript
npm run build --workspace cordis-sdk --prefix sdks/typescript
node --test tests/typescript-sdk/terminal-ownership.mjs tests/typescript-sdk/queued-timeout.mjs
python3 -m pip install -r sdks/python/requirements.txt
PYTHONPATH=sdks/python python3 -m unittest discover -s tests/python-sdk -p 'test_cancellation.py' -v
python3 tools/conformance_run.py --output target/conformance-report.json
```

组件一致性工作流自动触发这些证明和实现检查，并保存 `formal-semantics.json` 与 `conformance-report.json`。前者记录模型、公理审计、源程序桥接、负控和源码摘要；后者记录适用上游场景的真实执行证据。持久化测量记录与单次运行数据应连同对应源码摘要使用。
