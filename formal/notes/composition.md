# 跨子系统资源交接与恢复

`Composition.lean` 把 UP 首次写入的提交权与 `Routing.Batch` 的出口引用连接为同一转换系统。一个 `Transfer` 对应一次出站请求及其捕获的批次，状态保存写入标志、目录、终结权、等待执行的 commit/rollback 回调和已执行次数。

## 从非空批次到提交或回滚

`prepare` 调用路由出口分配规则，将新引用记入这个回调持有的批次。写入令牌已经交出或请求已经终结时，准备新引用的请求保持原状态。路由的连接存活、加载代、容量和有限身份条件沿用 `Routing` 的相应契约。

首个正长度写入在 `RequestWrite` 临界区取得提交权，模型将它加入 `commitDue`；首写之前取消将它加入 `rollbackDue`。回调执行和销毁位于随后的独立事件，因而临界区之间的间隔也包含在证明中。每条可达轨迹满足：已提交、待提交、已回滚、待回滚及尚未交出的令牌总数为一。

提交移除批次标记并保留每个出口引用。回滚将仍属于这个批次的目录引用移入锁外待销毁集合；后续销毁事件才增加实际释放计数。任意准备、提交、回滚、重复释放、换代及关闭的组合保持逐身份引用守恒。`reachable_nonempty_batch_commits` 和 `reachable_nonempty_batch_rolls_back` 给出任意有效身份上限与 owner 下的非空可达结果。

`directory::tests::outgoing_batch_cancellation_observes_first_positive_write` 使用真实 `Router::export`、`Batch::outgoing`、`Connection::request` 和调用任务取消。可控写端先返回 Pending；第二种情形仅放行一个字节。首写前取消释放出口，首写后取消保留已提交出口直到显式 release。路由析构器同时检查目录锁可取得，覆盖回调销毁与锁外释放的实际交接。

## 全局事件与构造日志恢复

全局状态保留权威事实、按身份索引的资源、上述请求批次、清理工作及存储程序。事实和独立资源动作具备明确的保持条件。清理完成只释放 `cleanupResource` 对应的 `cleanupOwner`，并要求对应工作队列已经终结；直接的资源完成动作无法绕过这条联动规则。

`global_storage_trace_projection` 证明从全局轨迹删除非存储事件后，存储状态恰为 `StorageProgram.run` 的结果。`global_crash_recovery_is_atomic` 据此调用完整源码事务的日志恢复定理：恢复从运行中实际构造的 journal 读取原字节，结果为原始完整映像或最终完整映像。默认备份名列表来自初始写计划，因此修改名覆盖条件直接由构造满足。

这些定理量化所声明事件的任意有限轨迹。接口原语、有限身份、锁和调度的实现对应关系由各域源码桥接提供。`formal/bridges/composition.json` 记录生产入口、定理与回归命令。

## 转移身份

`Architecture.Resource.nextRequest` 在 owner 变化后保留单调前沿。发送只接受尚未使用的标识，并把前沿推进到该标识的后一位；任意后续事件都无法降低前沿。`old_callbacks_cannot_match_a_later_transfer` 将这项归纳性质与接收/取消的标识检查组合，排除跨多次转移的旧回调重合。

模型使用自然数。生产 UP 每条连接从一开始，以原子 `fetch_update` 和 `checked_add` 分配请求号，发送的编号范围为一至 `u64::MAX - 1`，耗尽返回现有错误码 8；出口身份分配同样检查溢出。连接身份隔开不同编号域。模型允许其他资源的分配使本资源看到的标识出现间隔，保留同一域内的单调新鲜性。
