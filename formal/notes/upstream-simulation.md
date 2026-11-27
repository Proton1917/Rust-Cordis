# 上游生命周期状态与迹模拟

`UpstreamSimulation.lean` 的共享状态保存目标纪元、当前尝试的纪元、错误、迁移占用与阶段、已登记效应、清理中的效应和 committed provider。TypeScript 刷新与完成分支按固定上游版本的控制流定义，Rust 分支调用已证明的生命周期决策。逐事件状态等价通过归纳扩展到任意有限轨迹。

Rust 状态额外保存待发布状态位。`statusPublished` 与 `executorYield` 在不产生外部操作时保持共享状态；擦除这些内部事件后，`arbitrary_microtrace_simulation` 证明 Rust 终态与 TypeScript 对应迹的终态相同。状态通知引起的目标变化或其他回调动作由各自的可见事件表示。

共享域包括活跃且未暂停的 owner、精确可比较的纪元身份、对应的 commit/collect/callback 事件。加载完成只处理本次 attempted 纪元；失败回调在目标已换代时保持新目标。卸载将非空效应按反向登记顺序交给清理，并在全部完成之后释放 committed providers。Rust 在清理期间新增顶层效应时的重试属于扩展语义，共享完成域要求该集合已经排空。

## 固定源码执行

`check_upstream.py` 验证上游检出为 `261c5fdae6edb54e7ed4f071024f530acd18903b`，提取 `_getState`、`_setEpoch`、`_reload`、`_unload` 的原方法体。仅擦除 `Promise<void>` 中的类型参数，其他方法体保持原样。

114 个输入包括原来的状态与目标观察、await 期间换代、陈旧失败，以及 32 个非空清理组合。清理组合控制回调登记数、异步完成、抛出错误、目标是否继续活跃、提供方是否被消费者阻塞，并观察 exclusive/shared provider 的成员删除和 release 次数。四个负控分别破坏惯性分支、陈旧失败判断、反向清理顺序和 provider 成员删除，均须被拒绝。

探针的服务集合、受控回调、错误组合器、日志接收和环境观察具有显式支持契约。任意迹定理作用于上述状态协议；有限源码执行将协议连接到当前方法体。完整 JavaScript/Rust 语言、平台调度和其余公共 API 行为保留各自的验证边界。运行 `python3 formal/check_upstream.py --evidence target/upstream-state-simulation.json` 取得绑定当前证明与源码摘要的结果。
