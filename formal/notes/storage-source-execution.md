# 存储调用顺序与恢复

## 程序状态与持久状态

`StorageProgram.lean` 把一次部署拆成各个可返回错误的文件操作：保存原文件、确认 journal、发布 journal、确认 journal 的父目录，以及逐文件的创建临时文件、写入、文件同步和名称发布。最后分别执行数据目录同步、移动决策标记、决策目录同步。删除状态文件使用单独的 remove 步骤。

`Machine` 保存当前调用位置、可见文件内容、持久文件内容、剩余写入、剩余备份和已构造的 journal。`advance_simulates_model` 将成功返回投影到 `Persistence.Step`；错误返回使该次调用停止。后台持久化单个名称由独立事件表示，所以任意调用前缀可以同时包含文件系统已经完成的部分持久化。

实际备份顺序是 instances、sources、BASE，再遍历状态键；实际写入顺序是 BASE、状态、instances、sources。`initialWithBackupOrder` 分别接收两个列表。`commit_deployment` 先编码得到一个 `PreparedDeployment`，取得部署锁，再把其中的状态键交给备份方法，并把同一个值借用给写入方法。`StorageControlFlow.source_entry_derives_backup_coverage` 从这个入口构造推导每个修改名称都出现在备份列表中。归纳证明给出 journal 中保存的每个原始值，`actualRecovery` 直接读取构造出的 journal，`complete_source_transaction_recovers_old_or_new` 证明恢复得到完整旧部署或完整新部署。

## 有序源码证书

`tools/storage_program_probe.py` 从当前 `storage.rs` 提取受支持的语句结构，包括完整提交入口、字段与路径绑定、备份循环、状态写入/删除分支、错误传播和决策确认。入口证书要求编码、加锁、以 `prepared.states.keys()` 备份以及以 `&prepared` 提交按源码顺序执行。提取器保留原调用顺序，并对未支持的结构返回失败。它生成 `Template` 的 Lean 定义及其等于 `canonical` 的证明；完整生成内容保存在执行 receipt 的 `control_flow_certificate` 中。

`StorageControlFlow.Execution` 在每个节点检查实际指令等于 `StorageProgram.nextIo`。`canonical_code_follows_every_io` 对任意长度的备份列表和写入/删除列表建立完整执行，`execution_prefix` 得到任意前缀，`certified_source_prefix_recovers_from_its_journal` 再与已构造 journal 的恢复定理组合。状态循环的规模由全称变量表示。

恢复提取器检查 `restore_deployment_files` 的完整结构：journal 存在性与版本检查、根目录和 consumed 目录遍历、目录项与版本文件的跳过条件、名称解析、缺失标记分支、原内容分支、每个目标目录的同步，以及最后的决策移动与确认。原内容分支实际执行临时链接清理、从 journal 创建硬链接、原子发布和清理；缺失标记分支删除目标名称。任何额外的名称过滤条件都会使结构提取失败。

`certified_file_restore_uses_journal_bytes` 证明一次成功的文件恢复采用 journal 中的原字节，`certified_recovery_loop_refines_replay` 对任意名称列表证明完整循环等价于 journal 重放。`certified_entry_transaction_recovers_old_or_new` 把真实入口构造出的两个计划、任意事务事件迹和这个恢复循环连接起来，覆盖条件由同一个 `Prepared` 推导。

这一连接包含两个可分别检查的部分：受限提取器把指定 Rust 语法结构映射成有序模板；Lean 内核验证模板解释的所有列表和前缀。真实目录项到模型名称列表的表示关系、Rust 的完整语法与别名规则、标准库实现及文件系统行为由源码执行证据和环境契约支撑。名称列表定理使用任意有效名称，实例矩阵提供当前源码在真实目录上的执行证据。

## 原样方法体的文件系统执行

`formal/storage_program_probe.rs` 提供支撑类型和 I/O 包装。探针把 24 个当前生产源码块原样插入后编译，包含 `commit_deployment`、`lock_deployment`、`prepare_deployment`、`commit_prepared`、`restore_deployment_files`、决策确认及目录初始化；生产方法体中的分支、循环和 `?` 保持原样。部署矩阵通过真实 `commit_deployment` 入口执行，文件锁调用真实标准库独占锁。包装层调用真实 `std::fs`、`File`、`Path` 和目录项，记录调用并在指定位置注入错误或退出。

六类目录夹具覆盖空状态列表、已有状态写入、状态删除、原先缺失的状态创建，以及多个写入/删除/创建混合，混合夹具包含 `z.state`。夹具保留一个不参与部署的文件检查 frame 条件。编码适配器返回一个承载有效夹具字节的 `PreparedDeployment`；备份和写入均由真实入口使用这个值。每次运行在编码完成后根据该 `HashMap` 的实际迭代顺序记录计划，Lean 独立产生该计划的 `nextIo` 序列。成功运行要求完整序列一致，故障运行要求故障之前的成功操作前缀一致。

当前矩阵包含 961 个逻辑案例，其中存储执行 952 个、目录初始化 9 个：

| 部分 | 案例数 | 检查内容 |
| --- | ---: | --- |
| 完整部署成功 | 6 | 新内容、完整调用顺序、未修改名称 |
| 部署各 I/O 返回错误 | 242 | 错误传播、回滚或恢复要求、确认时机 |
| 部署各 I/O 前退出 | 242 | 无栈展开退出后以新进程恢复 |
| 恢复成功和各 I/O 返回错误后重试 | 234 | journal 原内容、恢复幂等与错误传播 |
| 恢复各 I/O 前退出 | 228 | 新进程继续恢复到完整旧内容 |
| 目录初始化及逐同步点失败重试 | 9 | 子目录、根目录、全部祖先的确认顺序 |

470 个退出案例会启动独立恢复进程。退出发生在选定 I/O 之前；其余已完成的真实文件系统操作保留在临时目录中。恢复进程重新构造 `Store` 并运行同一原样恢复方法。

七个独立源码负控包括遗漏文件同步、数据目录同步之前移动决策标记、忽略写入错误、提交入口遗漏状态备份键、恢复时跳过 `z.state`、遗漏 consumed 子目录同步、忽略初始化同步错误。前五项分别由生产源码的控制流证书与编译后的执行检查独立拒绝；后两项由目录初始化顺序和失败传播检查拒绝。

## 目录初始化

`prepare_directory_tree` 创建 consumed/data，取得根目录的 canonical 路径，然后同步 consumed、data、根目录及每一级祖先。已存在目录同样执行这组屏障。`DirectoryInitialization` 记录剩余待确认目录和成功确认目录，证明成功确认始终是所需顺序的前缀，失败停止本次尝试，重试重新确认整个路径。

`children_and_ancestors_all_confirmed` 将初始化成功连接到全部子目录与祖先的持久性前提。探针对每个实际同步位置逐一返回错误，再在保留的现有目录上重试；源码实际顺序与 Lean 的 `nextIo`、失败状态、重试状态分别对照。

## 环境契约与复现

文件同步成功意味着指定文件数据完成持久化，目录同步成功意味着之前的名称变更完成持久化，rename 对单个名称原子发布。真实入口按源码取得并持有部署锁，其排他性由操作系统文件锁契约保证。原先已提交的数据和目录树已按初始化协议确认。物理断电、文件系统及存储设备对屏障的实现属于这些 OS 契约，执行矩阵覆盖进程退出与 I/O 错误后的恢复。

支撑适配器包括只保留消息的 `WireError`、空 cache clear、有效字节夹具的 `PreparedDeployment::encode` 和限定于固定有效夹具名称的校验器。字节序列化语义保留在对应生产实现的契约中。路径查询、canonicalize、metadata 和目录项底层读取使用真实标准库；故障枚举范围以 receipt 中记录的包装调用为准。目录项排序只固定可复现的故障编号。锁获取、journal 版本文件的发布和 staging 创建属于事务开始前的准备步骤，staging 删除属于确认后的清理步骤；这些步骤的包装 I/O 错误同样进入执行矩阵。

```sh
python3 tools/storage_program_probe.py --evidence target/storage-program.json
```

每次运行从源文件重新编译 Persistence、StorageProgram、DirectoryInitialization、StorageControlFlow，重新生成并检查有序源码证书，再编译原样 Rust 方法体和负控。receipt 原子更新为 running、passed 或 failed，记录全部输入的运行前后 SHA-256、源码块哈希、案例分类、初始化模型、实际状态列表顺序和负控结果。
