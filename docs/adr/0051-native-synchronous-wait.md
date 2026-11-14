# ADR-0051：原生插件的宿主协同同步等待

| 项 | 值 |
|---|---|
| 状态 | 设计方已批准（20d7db10），实现与受影响验收通过 |
| 日期 | 2026-11-15 |
| 依据 | DOC-12 §9、§13，DOC-15 CP-8，AGENTS 第一性原理第 3、5 条 |

## 问题与实测

cordis-host 为同组插件提供缺省两个线程的原生执行器。同步服务方法、ctx.spawn 任务及 Bridge::invoke 的 RPC 等待共用该执行器。SDK 已有同步代理经 NativeExecutor::wait 调用宿主 Tokio 的 block_in_place，允许执行器交出工作线程；插件自行执行 futures::executor::block_on 时，执行器继续把该线程视为运行中的任务。

真实双宿主用例 concurrent_raw_wait_exhausts_native_workers 已复现：提供方位于 wait-storage，同步使用方位于 wait-client，两个方法通过同一个 Barrier 同时进入跨进程异步调用。两次调用在 1001 ms 后均返回 code 6，第三次调用也返回 code 6，随后 wait-client 被回收。测试设置每次调用截止时间为 1 秒，保持既有连续三次截止回收规则。日志为 target/native-wait/evidence/reproduction.log；首轮参数编码问题另存 reproduction-invalid-arguments.log。

## 职责放置

执行线程及其工作队列由宿主掌握，因此线程交接入口位于宿主，经 ABI 提供给独立编译的插件。SDK 负责 Future 与回调的安全包装，结果保持调用方的 Rust 类型。内核继续决定实例与依赖生命周期，UP/1 沿用既有调用和取消协议，runtime 沿用既有宿主启动流程。线程容量属于部署事实，后续需要配置时由项目所有者决定排期，在 cordis.toml 按组声明并由 runtime 转成宿主参数。

## 决定

### SDK 入口

增加 `Context::wait<F, T>(&self, future: F) -> Result<T>`，其中 `F: Future<Output = Result<T>>`。Context 提供明确的宿主环境；Future 可以借用同步调用中的服务和参数，结果保持原 Rust 类型。调用者通过 `ctx.wait(secrets.get(name))` 等待已有异步接口。异步方法及后台异步任务继续优先使用 `.await`。

Future 在当前调用线程由 futures-lite 2.6.1 的 block_on 驱动，该实现支持递归等待并在源侧管理 Waker。进入驱动器之前，经 ABI 同步回调通知宿主把该线程上的执行器工作交给替代线程。宿主执行 block_in_place，回调返回后恢复原调度状态。回调与输出在调用方持有，借用期覆盖一次宿主调用；回调的 panic 经源侧 ABI 边界转换为 code 5。调用方保持 Future 的取消与库租约责任，跨进程截止时间及实例清理规则继续适用。

### ABI v1.1 追加能力

保留冻结的 HostV1 v1.0 前缀及 28 个既有槽。增加 `#[repr(C)] HostV1_1`，其首字段为 HostV1，表尾追加 `block_in_place(data, callback, err)`。callback 为 `extern C` 函数指针；data 是调用期间有效的借用指针。宿主在当前线程恰好调用一次 callback，待其返回后才结束入口，回调状态由调用方释放。

HostV1_1 首部 size 覆盖完整扩展表，ABI 次版本为 1。读取方先检查原始指针首字段的长度，再读取可用槽位；Environment 保存原始宿主表指针的来源范围，使尾部槽访问覆盖真实完整分配。对旧宿主的等待调用在执行 Future 前返回 code 3。新宿主接受旧插件；新 SDK 在旧宿主上继续提供 v1.0 能力。

兼容验收保存 docs/abi/v1-layout.txt 的 v1.0 布局，新增扩展布局记录；Miri 覆盖旧表长度、扩展表借用、回调 panic 及资源释放。宿主环境的原始表指针与只读表寿命遵循 DOC-12 §3 的握手契约。

### 宿主诊断

宿主可以观测一次 Future::poll 的持续时间、当前占用工作线程数及调用取消。单靠这些事实区分阻塞等待与同步计算存在歧义。因此诊断报告分组、占用线程数、持续时间，并提示排查自行 block_on 或长时间同步计算。

在实际调用取消时检查原生轮询状态，全部工作线程持续处于 poll 内至少 100 ms 时输出一次诊断；同一占满阶段复用记录，线程恢复后开启下一阶段。宿主自身的安全等待从占用统计中暂时移出，返回时恢复。诊断沿宿主 stderr 与既有分组日志转交，字段为分组、方法名、占用线程数和持续时间；业务参数与结果保留在调用链内。

### 分组线程数

线程数是部署事实，适合在 cordis.toml 按原生分组配置，并由 runtime 转成宿主已有的 --native-workers 参数。配置应使用分组名到正整数的映射，缺省保持 2，运行中修改通过已有分组重启流程生效。

按组线程配置已评估、暂缓，由项目所有者决定后续排期。其作用是调整并发容量，等待协同继续由宿主入口保证。本次交付 SDK/宿主的安全等待和诊断，runtime、supervisor、内核及 UP/1 的产品代码保持原样。

## 影响与验收

实现范围为 cordis-abi 的追加槽与安全包装、cordis-host 的调用与调度诊断、cordis-sdk 的 Context::wait，以及真实夹具、guide、rustdoc。cordis-sdk、cordis-abi、cordis-host 分别发布代码版本 0.2.1、0.1.1、0.1.1，宿主 ABI 为 1.1；等待入口要求 cordis-host 0.1.1 及以上，其能力由槽长度检查。既有核心 0.1.1 继续适用。

验收包含原始等待复现、安全等待的双并发结果、原生后台任务和系统线程调用、错误与 panic 传播、旧宿主能力诊断、Miri 与 v1.0 布局保持、既有原生宿主场景及 skill 模板编译。格式、受影响 crate 的 clippy 与矩阵、unsafe 审计沿项目入口执行。测试现场由 TestGuard 管理，构建沿 target/debug 与 target/shared。

新增正向场景在缺省两个原生线程下同时执行四次安全等待，全部在截止时间内完成，并覆盖嵌套 wait 与 ctx.spawn_thread 系统线程。旧宿主场景使用真实原生插件，调用 wait 返回 code 3，Future 的实际轮询计数保持 0。异步方法与 ctx.spawn 任务优先使用 await；wait 服务于同步调用上下文。

实际结果见 [同步等待验收](../measurements/native-synchronous-wait.json)：原生 16 项、ABI/SDK 23 项、模板 1 项通过，Miri 的 16 个边界用例及两份布局比较通过。四路安全等待实测 1 ms；真实旧宿主 0.1.0 返回 code 3，轮询数为 0。新宿主装载已发布 SDK 0.2.0 的 secrets 动态库通过。既有核心进程各 crate 的产品差异为空。
