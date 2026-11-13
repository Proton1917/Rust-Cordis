# 进度日志

本文件记录实现的当前位置，供中断后继续。每个里程碑结束时更新；里程碑进行中发生需要后来者知道的事情时也更新。更新时改写“当前状态”一节，并在“里程碑记录”中补充对应小节，不在文件末尾堆叠流水账。

## 当前状态

| 项 | 值 |
|---|---|
| 最近完成的实现与检查 | M22：HTTP 路由、通用字节流、页面调用来源、密钥服务与三语言脚手架；AU-19–AU-26、RM-11 |
| 当前里程碑 | M22 实现与验收完成，等待设计方复核；M21 已通过复核 |
| 当前验证 | 工作区 336 项用例（含 2 项文档用例）通过；RM-11 独立测量通过。fmt、clippy、177 条矩阵、unsafe、Miri 13 条边界及布局通过。初次失败按影响范围修复与补测，完整记录见 M22 节 |
| 下一步 | 等待 M22 设计方复核；通过后按 30f61b6a 发布 macOS arm64 v0.1.0、验证下载包并清理编译缓存，再按 ADR-0047 实施 M23。主会话独立执行，编译共用 target/shared |
| M22 边界 | 按设计方三项裁决：内核、up、channels、types、process 与 ABI 相对 df05064 保持；supervisor 先提取到 routing 再添加受信调用元数据，结束时至多 5450 代码行；web-deployment 限页面来源，runtime 限 service install |
| M22 受信信息 | 核心对 scoped 调用追加直接调用方 cordis.caller 与页面的 cordis.page；任何 intercept 来源提交 cordis.* 返回权限错误。secrets 以自己的 grants 配置结合可信调用方鉴权。机密由插件读取和解释，核心转交不透明载荷；AU-25/26 已搜索最详细日志、状态、检查点和错误中的测试值 |
| M22 页面授权 | 22518c32 裁决：后端 caller={instance}，页面 caller={page_of} 且省略 instance；两者描述直接调用方。secrets 只接受 instance 授权，后端处理页面调用时仍以自己的 instance 读取密钥。三语言 helper 与全栈场景按此验证 |
| M22 实际故障 | 独立线程 RPC 的 reactor 问题已按批准范围修复；Bridge::invoke 与 sleep/plugin/effect_wait/emit_async 进入宿主执行器。普通线程、自有 Tokio、取消与取消后继续调用场景通过，原失败和修复日志归档在 target/http-m22/evidence |
| 发布准备 | macOS arm64 六个程序/库的路径重映射 release 编译已通过，30.92 秒，产物在 target/shared/release，日志在 target/release-v0.1.0/evidence；标签、上传、下载验证与清理均待 M22 后的发布安排 |
| M20 评审跟进 | segmentation_fault_isolates_group 的等待修正及五轮稳定性验收已完成；supervisor 保持 5480/5500 代码行 |
| M21 性能结果 | RM-7 三方向各 10000 次，RM-8 三次替换完成。RM-9 首轮中位数比 1.07711183；停载重测中位数比 0.95774113、最大值比 0.90793525，满足原 1.05/1.15 判据。完整样本与测量条件并列归档 |
| 后续事项 | ADR-0045 静态分派候选保持暂停，保存在 target/phase3-acceptance/m21/rm9-candidate 与 rm9-analysis。两轮前 1–4 轮偏高、比值相差约 12%；预热长度与交替采样方法留作独立研究 |
| 清理 | 用户授权清理已执行，target 从 60G 降至 30G；代理编译根、主编译 incremental 和旧里程碑二进制已释放。具体路径与空间实测见 M21 小节 |
| 执行安排 | 用户要求关闭全部子代理，由主会话独立完成 M22 与后续 M23；编译沿默认 target/debug 与 target/shared |
| 后续顺序 | 555a5e1f：M22 提交并经设计方复核 → v0.1.0（M22 标签）与缓存清理 → 按 ADR-0047 实施 M23 动态调用 → 复核后 v0.2.0。ADR-0047 与设计方阶段表改动随 M22 提交 |
| 验收资料 | docs/measurements/m21-acceptance.md 汇总 AU-1–AU-18；性能、语言、完整运行时和 native 稳定性另有原始 JSON。网页状态与操作见 docs/design/flow/index.html |
| 文档交付 | README 三张表、guide、DOC-13/15、ADR-0043、skill、解释页与独立 HTML 已同步 |
| 窗口界面验收 | AU-17 已归档；用户运行记录为恢复 802 ms，截图、Cmd-Q、检查点与进程退出通过 |
| 最后更新 | 2026-11-14（Asia/Shanghai），M22 实现、完整验收与资料归档 |

## 环境

| 项 | 值 | 来源 |
|---|---|---|
| Rust 工具链 | 1.96.0 stable，`aarch64-apple-darwin` | 2026-11-11 `rustc --version` |
| Miri 工具链 | nightly 1.102.0-nightly（3ef0c22cd 2026-11-11）；miri 与 rust-src 已安装 | M8 的 12 条边界行为与布局比较通过；sysroot 位于 target/miri-sysroot |
| `UPSTREAM` | `cordis/`，commit `261c5fdae6edb54e7ed4f071024f530acd18903b` | 2026-11-11 `git -C cordis log -1` |
| Python | `conda run -n base python`，执行项目辅助检查 | 项目约定 |
| 依赖版本 | slotmap 1.1.1；parking_lot 0.13.0；futures-core / futures-util 0.3.34；thiserror 2.0.21；serde 1.0.229；serde_json 1.0.154；tokio 1.54.1 | M0：逐项 `cargo search <crate> --limit 1`；workspace 固定版本，按模块调用引入 crate |
| 硬件 | Apple Silicon；14 个逻辑处理器；48 GiB 内存 | `sysctl -n hw.memsize hw.logicalcpu` |

## 里程碑记录

### M0 工程骨架

- commit：`c2c4f83`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：`cargo test --workspace` 通过，两个空 crate 和文档测试共 0 条；`cargo build` 通过。
- X-4：`conda run -n base python tools/conformance.py check` 通过，177 条均为初始状态。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 24 行。
- X-7：已记录环境、依赖版本及 M1 入口。
- X-8：提交主题 `M0: 建立工程骨架与固定工具链`。
- 矩阵：M0 分配 0 条用例。
- 偏离：无。
- 遗留：无。后续继续构建，保留本项目 `target/` 缓存。

### M1 内核基座与同步效应

- commit：`fe905f3`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：`cargo test --workspace` 通过，共 9 条测试；`cargo build` 通过。覆盖局部释放幂等、嵌套效应元信息、失败与 panic 后恢复、跨线程重入、根所有权、视图继承、有序移除及确定性时间。
- X-4：`conda run -n base python tools/conformance.py check --through M1` 通过：已移植 1、已改写 3、初始状态 173。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 722 行。
- X-7：已更新当前状态、矩阵及接口草图。
- X-8：提交主题 `M1: 实现上下文视图与可回退同步效应`。
- 矩阵：本里程碑已移植 1 条、已改写 3 条、不适用 0 条。
- 偏离：无架构规则偏离。`defer`、`adopt`、释放任务通过 `Result` 交回错误；按 DOC-02 §6 同步接口草图。
- 遗留：核心服务按 M2–M5 接入根上下文；异步效应、子纤程与惯性调和属于 M3。继续构建，保留 `target/`。

### M2 事件

- commit：`b258d75`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：`cargo test --workspace` 通过，共 32 条测试，事件测试 23 条；`cargo build` 通过。覆盖五种派发、四模式的删除与新增快照、一次性递归与并发调用、同步与异步瀑布、过滤、聚合错误、登记接管及派发观察。
- X-4：`conda run -n base python tools/conformance.py check --through M2` 通过：累计已移植 8、已改写 20、不适用 1、初始状态 148。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 1335 行。
- X-7：已更新当前状态、矩阵、接口草图及 ADR-0006。
- X-8：提交主题 `M2: 实现类型化事件与可回退监听器`。
- 矩阵：本里程碑已移植 7 条、已改写 17 条、不适用 1 条。
- 偏离：无架构规则偏离；ADR-0006 确认三种派发族及异步瀑布返回值。
- 遗留：纤程专属的 internal/update 行为随 M3 接入。继续构建，保留 `target/`。

### M3 组件生命周期

- commit：`7b96587`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：`cargo test --workspace` 通过，共 77 条单元及集成测试、2 条 compile_fail 文档测试；`cargo build` 通过。覆盖加载与卸载惯性、配置更新合并、失败恢复、丢弃等待句柄、步骤边界取代、异步逆序清理、嵌套组件、配置问题路径及状态监听器 panic 后继续调和。
- X-4：`conda run -n base python tools/conformance.py check --through M3` 通过：累计已移植 8、已改写 70、不适用 1、初始状态 98。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 2395 行。
- X-7：已更新矩阵、接口草图、Scope 租约说明及当前状态。
- X-8：提交主题 `M3: 实现惯性生命周期与异步效应`。
- 矩阵：本里程碑已改写 50 条；非法组件与非法效应值通过 compile_fail 文档测试检验公开接口的类型约束。
- 偏离：ADR-0007 规定 Fiber 持有所属 Scope，已失活记录随最后一个相关句柄回收。
- 遗留：依赖纪元、绑定可用性与依赖方等待在 M4 接入；内核错误当前集中记录，M5 接入日志服务。继续构建，保留 `target/`。

### M4 反应式余效应

- commit：`73b5646`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- X-3：首次 `cargo test --workspace` 通过，120 条运行测试与 2 条文档测试。随后用原始 CMP-10 触发复现调度差异，并以新增用例复现 InternalService panic 中断提供；修复后 `cargo test -p cordis --test component --test fiber --test update --test events_lifecycle --test reflect --test service --test service_dispose --test isolate` 的 83 条受影响测试通过，其余有效结果复用。当前共 121 条运行测试与 2 条文档测试；`cargo build` 通过。
- X-4：`conda run -n base python tools/conformance.py check --through M4` 通过：累计已移植 8、已改写 116、不适用 2、初始状态 51。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 3142 行。
- X-7：已更新矩阵、当前状态及 S-FBR-12 的调度步骤。
- X-8：提交主题 `M4: 实现反应式服务与依赖清理顺序`。
- 矩阵：本里程碑已改写 46 条、不适用 1 条。RFL-01 检验 JavaScript 子类的 Context.is 动态识别，Rust 使用 Context 类型及 Debug 映射，记录为不适用。
- 偏离：依据上游源码补正 S-FBR-12；架构规则继续沿用。
- 遗留：日志、拦截及组件替换在 M5 接入。继续构建，保留 `target/`。

### 并发评审修复

- commit：`b8c5756`，已推送至 `origin/main`。
- 入队标志与修订号、存活状态及依赖视图同锁更新；等待者在队列与迁移都结束后收到结果。调和动作和任务持有 Scope，记录回收遵循实际任务生命周期。
- 宿主提交前拒收与接收后 panic 分别处理。加载拒收经过失败卸载恢复；清理任务限定重新提交一次，后续拒收保留 Future 并交回错误，显式释放可继续推进。
- 用户提供的竞态用例输出 `settled_early=false`、`applied_final=2`。新增的宿主异常及恢复用例共 6 条通过；受影响的生命周期、服务、同步与异步效应测试共 95 条通过。末两处状态修正仅复测对应两条用例，均通过。
- 隔离快照位于 `target/review-fix`，以 `b721de4` 为基线；构建、Clippy 与格式检查通过，内核 3498 行。主目录同时保留 M5 实现，纯评审变更单独暂存。
- UPD-05 断言错误数恰为 1；服务重载、并发等待者、最终事件与隔离域用例保留上游次数约束。Scope 生命周期记录编号为 ADR-0007。
- 基础提交 `b721de4` 已将根 README、MIT 许可证和 crate 元数据推送至 `origin/main`；许可证正文沿用用户提供内容。

### M5 拦截、调用方句柄、日志、替换

- commit：`a358e0e`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过；后续日志来源修复与 RPL-12 断言调整分别通过对应库、测试目标的 Clippy。
- X-3：`cargo build` 通过。工作区测试完成已通过部分后，对 RPL-12 保留上游的次数及配置约束；随后 `cargo test -p cordis --test replace --test service --test service_dispose --test shadow --test update` 的 63 条测试及 `cargo test --workspace --doc` 的 2 条文档测试通过。宿主同步完成后 panic 的日志来源用例已复现错误并修复通过。当前共 179 条运行测试与 2 条文档测试，成功证据按受影响范围复用。
- X-4：`conda run -n base python tools/conformance.py check --through M5` 通过：累计已移植 8、已改写 165、不适用 3、初始状态 1。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 3995 行。
- X-7：已更新矩阵、接口草图、ADR-0008 与根 README 进度。
- X-8：提交主题 `M5: 完成服务调用上下文与组件替换`。
- 矩阵：本里程碑已改写 49 条、不适用 1 条。ASC-06 检验 JavaScript 类函数源码字符串及代理参数透传，Rust 的类型化 Debug 按映射承担对象展示。
- 偏离：ADR-0008 明确 Scoped 的 caller、definition 与 requester，保留效应归属、定义点访问和使用点拦截语义。
- 日志：默认环形缓冲上限 1000 条；超额移除最旧消息。错误记录和公开日志缓冲使用同一数据源，导出器按名称与级别独立过滤。
- 遗留：M6 交付示例、多线程运行证据与公开文档。继续构建，保留 `target/`。

### M6 示例、多线程与公开接口

- commit：`e459aa7`。
- X-1：`cargo fmt --all --check` 通过。
- X-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过；通知完成条件修复后，对库、并发和压力目标的定向 Clippy 通过。
- X-3：工作区测试首次运行在压力目标复现通知窗口；修复后原定 `eight_workers_recover_one_thousand_rounds` 以 8 个工作线程完成 1000 轮，耗时 2.82 秒。确定性通知屏障用例通过；随后 119 条受影响的生命周期、并发、服务、替换和日志测试及 DEC-01 用例通过，其余成功证据复用。当前覆盖 182 条运行测试与 2 条编译失败文档测试。
- X-4：`conda run -n base python tools/conformance.py check --through M6` 通过：177 条中已移植 8、已改写 166、不适用 3，初始状态 0。
- X-5：`rg -n unsafe crates/cordis/src` 输出为空。
- X-6：内核源码 4483 行，各模块见下表，满足 6000 行上限。
- X-7：已记录完整接口清单、执行结果和遗留状态，根 README 示例与进度同步。
- X-8：提交主题 `M6: 交付示例与多线程验收`。
- 示例：`cargo run -p cordis --example plugins` 按五步输出 Pending → Active → Pending → Active，绑定纪元依次为 `[1]`、`[2]`，根释放后四项快照恢复 `(0, 0, 0, 0)`；消费者清理先于提供方。`cargo run -p cordis --example readme` 输出与根 README 的六行一致。
- 文档：`RUSTDOCFLAGS='-D warnings -W missing_docs' cargo doc --workspace --no-deps` 通过。公开类型、字段、关联项及方法具备说明；根上下文显式释放及循环引用的处理已记录。
- 理论条款：DOC-01 的 34 条 T-* 均在测试注释中引用。
- 资源：示例二进制保存在 `artifacts/bin/`，生成的 API 文档保存在 `artifacts/api/`。已执行 `cargo clean -p cordis -p cordis-testkit`、`cargo clean --doc` 及隔离评审目录的 `cargo clean`；原评审资料目录继续保留。
- 偏离：通知计数补齐稳定判定，架构规则和接口语义沿用既定要求。
- 遗留：无。

| 内核模块 | 行数 |
|---|---:|
| context.rs | 222 |
| effect.rs | 506 |
| error.rs | 67 |
| events.rs | 658 |
| fiber.rs | 948 |
| kernel.rs | 147 |
| lib.rs | 38 |
| list.rs | 57 |
| logger.rs | 415 |
| reflect.rs | 708 |
| registry.rs | 539 |
| service.rs | 178 |
| 合计 | 4483 |

## 第二阶段里程碑

### M7 跨编译边界的内核入口

- commit：本节所在的 `M7: 实现拥有型名称与命名插件入口` 提交。
- Y-1：`cargo fmt --all --check` 通过。
- Y-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- Y-3：`cargo test --workspace` 通过，193 条运行测试与 2 条文档测试；10 条新用例覆盖 K-1–K-4，8 线程 1000 轮压力测试组耗时 3.03 秒。`cargo build` 通过。
- Y-4：`conda run -n base python tools/conformance.py check --through M6` 通过，177 条处置保持 8 条移植、166 条改写、3 条不适用。
- Y-5–Y-7：按 DOC-14 从 M8 开始执行。
- Y-8：内核 4903 行，testkit 181 行。内核高于 4800 行目标、低于 6000 行上限；新增行主要用于命名事件入口、共享派发辅助函数和运行时名称所有权。
- Y-9：已更新当前状态、接口细化与模块行数。
- Y-10：提交主题 `M7: 实现拥有型名称与命名插件入口`，推送至 `origin/main`。
- 设计落实：服务按名称统一解析，提供登记保留 typed/named 冲突；命名元数据后层覆盖；类型化事件保留 TypeId 身份，命名同步/异步族各自派发。同步命名回调通过 Result 传播业务错误。
- K-4：别名保存 ComponentId → ComponentId，终点定义表保留当前目标；连续三次替换的旧闭包 Weak 释放与旧别名重新挂载均通过。
- 遗留：M8 草案与候选继续按依赖顺序接入，当前保留构建缓存。

| 内核模块 | M7 行数 |
|---|---:|
| context.rs | 251 |
| effect.rs | 506 |
| error.rs | 67 |
| events.rs | 856 |
| fiber.rs | 1034 |
| kernel.rs | 155 |
| lib.rs | 38 |
| list.rs | 57 |
| logger.rs | 415 |
| reflect.rs | 753 |
| registry.rs | 566 |
| service.rs | 205 |
| 合计 | 4903 |

### M8 ABI v1.0

- commit：本节所在的 `M8: 冻结原生插件 ABI v1.0` 提交。
- Y-1：`cargo fmt --all --check` 通过。
- Y-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- Y-3：`cargo test --workspace` 通过，207 条运行测试、2 条文档测试。最终描述符 fixture 的指针发布调整与布局基准读取调整分别通过定向测试；其余结果复用。`cargo build`、`cargo +1.85.0 build -p cordis-abi` 通过。
- Y-4：`conda run -n base python tools/conformance.py check --through M6` 通过，177 条处置保持 8 条移植、166 条改写、3 条不适用。
- Y-5：源码审计通过。syn 与 proc-macro2 位于 ABI 的开发依赖；审计覆盖 unsafe 块、函数、impl、extern 块、属性和宏模板的紧邻 SAFETY 注释。
- Y-6：`cargo +nightly miri test -p cordis-abi` 的行为与布局目标通过，按失败修复范围分别执行并复用成功结果。12 条边界行为覆盖所有权、源侧释放、租约、跨线程唤醒、取消、错误与 panic、回调和描述符扩展；审计测试通过 `cfg(not(miri))` 留在原生执行。运行环境使用工作区内 MIRI_SYSROOT 与 TMPDIR。
- Y-7：`cargo test -p cordis-abi --test layout` 通过。布局记录包括 HostV1 的 22 个槽和全部 ABI 类型；Miri 通过编译期嵌入的同一基准逐字段比较。
- Y-8：内核 4903 行、testkit 181 行、ABI 2127 行。ABI 高于 1500 行目标、低于 2500 行上限；拥有值、错误、Future/Waker、描述符和回环测试支持分别维护源侧资源所有权。
- Y-9：已记录验证结果、Miri 工具链及 M9 入口；DOC-12 状态为已冻结 v1.0。
- Y-10：提交主题 `M8: 冻结原生插件 ABI v1.0`，推送至 `origin/main`。
- 设计决定：ADR-0015 规定统一错误通道、创建时取得租约、释放回调返回后归还租约、清理期上下文、状态导出解冻及组件 size 步进。
- 遗留：M9、M10 候选按下表继续接入，构建缓存保留。

### M9 Rust SDK 与通用调用

- commit：本节所在的 `M9: 实现 SDK 与类型化及通用插件调用` 提交。
- Y-1：`cargo fmt --all --check` 通过。
- Y-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- Y-3：`cargo test --workspace` 通过，209 条运行测试与 2 条文档测试；8 线程 1000 轮测试耗时 3.03 秒。SDK 回环通过真实入口覆盖 25 个 Host 槽、类型化与通用调用、接口追加、事件字段兼容、状态继承及失败后的资源释放。
- 构建：`cargo build --workspace` 通过，1.70 秒；`cargo +1.85.0 build -p cordis-sdk -p counter -p kv-mem` 通过，5.90 秒。AC-8 的真实跨编译器装载在新 M11 验证。
- Y-4：本阶段执行 `conda run -n base python tools/conformance.py check --through M6` 通过，177 条处置保持 8 条移植、166 条改写、3 条不适用；内核源码保持 M7 状态。
- Y-5：工作区的 unsafe 审计测试通过，0.27 秒；允许位置改为 ABI、cordis-host 与宏固定模板。核心侧 crate 的源码继续使用安全 Rust。
- Y-6：`cargo +nightly miri test -p cordis-abi` 通过；13 条边界测试 3.67 秒，布局比较 0.90 秒。覆盖通用资源的同步、异步、取消、未解码结果及源侧释放。
- Y-7：新布局记录已生成，工作区中的 `abi_v1_matches_layout_record` 比较通过；DOC-12 为候选 v1.0，按 DOC-15 在 M11 冻结。
- Y-8：内核 4903 行、testkit 181 行、ABI 2583 行、SDK 714 行、宏 1232 行。ABI 含 230 行测试桥，产品部分 2353 行；ADR-0022 将其目标与上限调整为 2500/3000。宏高于 1200 行目标、低于 2000 行上限，新增通用分派、资源参数与可选值编码。
- Y-9：DOC-12/13、ADR-0017 和当前状态已按 Q0 更新；设计方的 DOC-15、ADR-0018–0020、AGENTS 与路线图修改纳入本提交。
- Y-10：提交主题 `M9: 实现 SDK 与类型化及通用插件调用`，推送至 `origin/main`。
- 设计细化：ObjectHeader 同时描述类型化与通用入口；CBOR 参数数组、单值返回、unit 的 null、未知方法 code 3；资源使用标签 60000，嵌套可选值使用标签 60001。Host 资源表与 AbiBuf owner 明确调用期、结果和导入对象的引用寿命。
- 遗留：M10 协议与监管、新 M11 插件宿主分别在 target 候选中推进。系统的唯一内核位于核心进程，宿主使用普通结构与 RAII 管理本地资源。

### M10 UP/1 与核心监管

- commit：本节所在的 `M10: 实现 UP/1 与唯一核心的进程监管` 提交。
- Y-1：`cargo fmt --all --check` 通过。
- Y-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过。首轮指出的可见性、迭代与分支样式问题已修正。
- Y-3：`cargo test --workspace` 通过，235 条运行测试与 2 条文档测试，完整输出在 target/m10-tests.log。核心监管 6 条实际进程用例全部通过，22.74 秒；进程传输和组回收 3 条通过，0.32 秒；8 线程 1000 轮内核用例 3.03 秒。`cargo build --workspace` 通过，3.05 秒。
- Y-4：`conda run -n base python tools/conformance.py check --through M6` 通过，177 条处置保持 8 条移植、166 条改写、3 条不适用。
- Y-5：安全审计通过；核心监管的产品依赖树与 ABI、SDK 分离，记录在 target/m10-core-dependencies.txt。
- Y-6：本阶段 `cargo +nightly miri test -p cordis-abi` 通过，13 条边界行为 3.47 秒、布局比较 0.90 秒；继续使用已记录的 nightly 与工作区内 sysroot/TMPDIR。
- Y-7：新增 effect_release 后已更新布局记录；正式工作区与 Miri 中的布局比较通过，ABI 保持候选 v1.0。
- Y-8：内核 5101 行、testkit 181 行、ABI 2595 行、SDK 738 行、宏 1232 行、UP/1 1508 行、supervisor 3664 行。UP/1 高于 1500 行目标 8 行；supervisor 的目标与上限按 ADR-0026 为 3500/4500，当前所有 crate 均在上限内。
- Y-9：DOC-10–16、ADR-0009/0017 已按最新设计同步，新增 ADR-0023–0027；设计方的 AGENTS、CLAUDE 与 explainer 文件随本提交保存。
- Y-10：提交主题 `M10: 实现 UP/1 与唯一核心的进程监管`，推送至 origin/main。
- CP-9 正式提交复核：从已推送的 98394f9 导出隔离源码，直接构建其中 crates/cordis-up/fuzz 工程的 up1_decoder 目标；固定 max_total_time=1800，实际执行 1801 秒、1203602 次输入，退出 0，cov 2369、ft 6838、RSS 747MiB。完整输出在 target/m10-review/fuzz.log，源文件位于 target/m10-review/source/crates/cordis-up。命令为 `RUSTUP_TOOLCHAIN=nightly TMPDIR="$PWD/target/tmp" target/tools/bin/cargo-fuzz run --fuzz-dir target/m10-review/source/crates/cordis-up/fuzz --target-dir target/m10-review/fuzz-build up1_decoder "$PWD/target/up-m10/fuzz/corpus/up1_decoder" -- -max_total_time=1800 -timeout=2 -rss_limit_mb=2048 -max_len=16777216 -artifact_prefix="$PWD/target/m10-review/fuzz-artifacts/"`。此前 632926 次/1801 秒为候选实现证据，保留在 target/up-m10/fuzz/result.md。配置、清单和状态读取的 fuzz 随 M12 产品路径验收。
- 偶发失败复核：98394f9 隔离副本连续运行 cargo test -p cordis-supervisor 共 30 轮，全部退出 0；第 21–30 轮并发执行 cargo test -p cordis --release，共 136 次 release 运行全部成功。逐轮结果在 target/m10-review/stability/results.json，完整日志按 round 编号保留。本轮未复现评审所见的一次失败，现有证据不足以确定该次用例名及原因；既定 30 轮结束后停止追加轮次。
- 真实行为：核心统一决定依赖与清理顺序；事件回调可通过同连接调用服务；失败替换恢复旧状态；取消等待保持事务完成；SIGKILL 后撤回服务并继承最近快照；五次指数退避后停止重启；额度释放后继续登记。前端转发与派生组合按 DOC-15 保留对应阶段范围。
- 资源回收：ADR-0027 使用安全 waitid 的 NOWAIT 观察，单一任务在保有 PID 时发送组信号，再由 Child.wait 唯一回收。自然退出、SIGKILL 及存活后代清理均通过；僵尸组 EPERM 在回收后以只读组查询确认为空。
- 遗留：M11 基础宿主候选及 M12 前置解析候选继续按下表接入，构建缓存保留。

### M11 原生插件宿主、组合接口与热替换

- commit：本节所在的 `M11: 实现原生宿主与组合热替换并冻结 ABI` 提交。
- Y-1：`cargo fmt --all --check` 通过；Host 的安全注释位置修正后由 rustfmt 格式化对应文件。
- Y-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过，11.92 秒；随后补充提交事实边界，受影响的 supervisor 定向 clippy 通过，2.06 秒。
- Y-3：242 条运行测试与 2 条文档测试通过。首轮工作区测试通过内核与 ABI 的 213 条运行测试后，在 Host 安全注释位置审计处失败；修正后审计 1 条通过，其余工作区包的 28 条运行测试和内核 2 条文档测试全部通过，复用前段有效结果。命令与完整输出分别在 target/m11-tests.log、target/m11-audit.log、target/m11-tests-remaining.log、target/m11-doc-tests.log。`cargo build --workspace` 通过，1.80 秒。
- 原生验收：正式 `crates/cordis-host/tests/native.rs` 自行构建 fixtures 与 Rust 1.85 使用方，55.04 秒通过 13 个真实场景：同组 typed 与跨组通用调用 100→102、两种分组中的 Scoped/隔离/子组件、8 线程换库与状态 137、失败回滚/不兼容拒绝、旧租约归零、固定 21 次阈值重启，以及服务 panic、apply panic、abort、真实内存段错误、栈溢出、进程退出、死循环隔离。致命故障后的依赖方转 Pending，其他分组纪元保持。
- Y-4：`conda run -n base python tools/conformance.py check --through M6` 通过，177 条处置保持 8 条移植、166 条改写、3 条不适用。
- Y-5：`conda run -n base python tools/unsafe_audit.py` 通过，0.43 秒。核心监管产品依赖与 ABI/SDK 分离；Host 产品依赖与内核分离，记录在 target/m11-core-dependencies.txt 与 target/m11-host-dependencies.txt。
- Y-6：`MIRI_SYSROOT="$PWD/target/miri-sysroot" TMPDIR="$PWD/target/tmp" cargo +nightly miri test -p cordis-abi` 通过，13 条边界行为 3.41 秒，布局比较 0.94 秒。
- Y-7：AbiContext 与 28 槽 HostV1 的布局已生成，原生工作区与 Miri 中的布局比较通过；DOC-12 冻结为 v1.0。
- Y-8：下表按 `conda run -n base python tools/source_lines.py` 统计。cloc 2.10 使用 Rust 注释规则，排除空行、普通注释与文档注释；代码和注释共行时归代码。参数 `--skip-uniqueness` 逐文件计数，`--sdir=target/tmp` 保持中间文件在工作区。supervisor 的组合、组内换库和原快照回滚接口经职责拆分后为 4562 代码行，按 ADR-0030 将目标/上限调整为 4500/5500，交设计方复核。UP/1 和 Host 高于各自目标，低于上限。
- Y-9：DOC-12/13/16、ADR-0012/0026/0028–0030、README 和当前状态已同步；设计方新增项目规则随提交保存。
- Y-10：提交主题 `M11: 实现原生宿主与组合热替换并冻结 ABI`，推送至 origin/main。
- 设计细化：Service 使用共享接口对象与 ABI 对象，Rust 1.85 原始 Service 跨 await 场景及 watch 的自然 async 函数构建通过；运行时与 SDK 共用 cordis-types。cordis-process 统一进程组生命周期与受限子进程探测。
- 生命周期观察点：按 T-LIF-8、S-FBR-12/13，父实例 dispose 完成时全部子实例已结束，私有域服务已撤回；顶层效应逆序启动并发清理。原生验收以实例完成与服务回收为判据。
- 状态提交：手动恢复接口保存 SW-5 的原快照；替换错误的外层 committed 由核心事务独立持有，插件错误置于 cause，运行时据此维持已切换部署与持久化的一致性。
- 遗留：M12 运行时、watch 与脚手架候选继续集成；构建缓存保留供后续里程碑使用。

| crate | 代码行 / 总行 |
|---|---:|
| cordis | 4222 / 5101 |
| cordis-abi | 2285 / 2700 |
| cordis-host | 2345 / 2602 |
| cordis-process | 374 / 410 |
| cordis-sdk | 687 / 793 |
| cordis-sdk-macros | 1179 / 1280 |
| cordis-supervisor | 4562 / 4912 |
| cordis-testkit | 161 / 181 |
| cordis-types | 156 / 177 |
| cordis-up | 1613 / 1752 |
| 六个核心 crate 合计 | 10927 / 12352 |

### M12 核心命令、配置调和与批次管理

- commit：本节所在的 `M12: 实现核心运行与持久化批次管理` 提交。
- 交付：正式 cordis-runtime、cordis-dev、watch、console-log、kv-file 与原生部署示例；UPSTREAM-EXT 规则见 ADR-0031–0033。管理接口 1.1、Runtime 1.2 提供逐方法授权、叠加层、scope/disabled、原状态回滚、整窗口替换和持久化批次结果。
- Y-1：`cargo fmt --all --check` 通过；仅修复开发工具 UUID 配置布局和监管器模块顺序的格式差异。
- Y-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过，4.13 秒；随后修复关闭竞态，受影响的 supervisor 定向 clippy 通过，1.22 秒。
- Y-3：275 条运行测试与 2 条文档测试通过。工作区执行复用已成功的 watch 用例；首轮到 supervisor 的重启上限用例时发现关闭竞态，其余 258 条运行测试通过。修复该失败用例后 1 条通过，17.95 秒；后续 process 4 条、其余包 11 条、watch 1 条和内核 2 条文档测试全部通过。日志为 target/m12-workspace-tests.log、m12-shutdown-race.log、m12-process-tests.log、m12-tests-remaining.log、m12-watch-self-confirmed.log、m12-doc-tests.log。`cargo build --workspace` 通过，4.70 秒。
- 关闭竞态：失败分组的连接已结束，状态快照先于排队回收显示 Failed；shutdown 取消排队回收后，旧纤程仍处于 Active，导出路径向已结束连接发送请求。导出资格现同时检查连接存活，已断源沿 CP-6 保留最近检查点，已开始导出的真实错误继续返回。当前失败位置为 restart_limit_uses_backoff_and_latest_snapshot 的最终 shutdown；M10 评审中那次未记录用例名的失败继续保留原证据范围。
- 实际管理：原状态 46 与候选 91 的手动回滚、可变基础来源覆盖后的原副本恢复、137 跨核心 SIGKILL 恢复、空原快照、退役失败后的已提交状态，以及提供方和使用方共同升级 counter@1→@2 均通过。不同组共用基础来源时分别保存 0.2.0/0.1.0，并在坏基础来源下重启恢复；后续配置更新仍可把真实服务值改为 73。
- 实际 watch：整窗口候选及配置共同提交，失败同时恢复；Ticket 在执行前持久化，实例重建后接续待确认窗口；相同被拒内容保持当前版本。自身失败场景曾复现基础库路径覆盖后的重启失败，修复为独立的按实例声明来源与已确认副本关联；正式完整场景通过，20.83 秒，包含坏基础 watch 库覆盖后的核心重启。
- 原生宿主的 13 个场景现各有独立测试名，fixtures 通过 OnceLock 只构建一次，正式测试通过，9.73 秒。核心实际原生示例的最终状态为 stopped；console-log 已验证等级、标签、多行、截断、时间和过滤，实际输出在 target/runtime-m12/build/console-live/output.log。
- Y-4：`conda run -n base python tools/conformance.py check --through M6` 通过，177 条处置保持 8 条移植、166 条改写、3 条不适用。
- Y-5：工作区 unsafe 审计测试通过。`cargo tree -p cordis-runtime --edges normal` 的产品依赖保持 Q0 边界，记录在 target/m12-core-dependencies.txt；文件监视、时间格式化与随机请求标识分别由插件持有。
- Y-6–Y-7：使用工作区 MIRI_SYSROOT/TMPDIR 的 `cargo +nightly miri test -p cordis-abi` 通过，13 条边界行为 3.41 秒、布局比较 0.94 秒；工作区原生布局比较也通过，ABI 继续为 v1.0。
- MSRV：`cargo +1.85.0 build -p cordis-watch -p kv-file -p console-log --target-dir target/m12-msrv` 通过，6.24 秒。
- CP-9：configuration-final 15151009 次 / 1801 秒 / exit 0；plugin_manifest-final 14687854 次 / 1801 秒 / exit 0；state_block-submitted 18529539 次 / 1801 秒 / exit 0。完整命令和状态在 target/runtime-m12/fuzz/logs 对应 JSON 与日志，限制保持 2048 MiB、单输入 2 秒、16 MiB。state 的旧候选曾触发 ASan 常驻内存上限，0 字节产物单次复放正常；最终运行采用 ASAN_OPTIONS=malloc_context_size=5，保留全部检测。M10 UP/1 解码器的 98394f9 正式证据独立保留。
- 源码对应：configuration 快照比较 config.rs、config/scopes.rs；plugin_manifest 比较 manifest.rs、config.rs、config/scopes.rs；state_block 比较 storage.rs、config.rs、config/scopes.rs。命令形式为 `cmp target/m12-fuzz-tested/<目标>/src/<模块> crates/cordis-runtime/src/<模块>`，三组均退出 0。正式 fuzz 工程保存在 crates/cordis-runtime/fuzz。
- Y-8：下表按 cloc 2.10 的 src 代码行口径统计。runtime 经职责拆分后为 4304 代码行，按 ADR-0034 调整目标/上限为 4500/5500；supervisor 为 5463 代码行，保持 ADR-0030 的 5500 上限。六个核心 crate 为 cordis、cordis-supervisor、cordis-runtime、cordis-up、cordis-process、cordis-types，按各阶段实际存在的实现求和。
- Y-9：README、DOC-02/11/13/15、ADR-0017/0031–0034、解释页与当前状态已同步。解释页的实际桌面与移动进度区域截图为 target/explainer-m12-desktop.png、target/explainer-m12-mobile.png；检查后浏览器与本次服务已退出。
- Y-10：提交主题 `M12: 实现核心运行与持久化批次管理`，推送至 origin/main。
- 后续：M13 候选已有 kv-mem/kv-file 的两阶段真实契约成功、违约与崩溃失败及宿主验证门证据，下一步正式接入并完成运行时替换验收。后续范围按更新的项目规则持续至 M21，M15 skill 同属交付。

| crate | 代码行 / 总行 |
|---|---:|
| cordis | 4230 / 5112 |
| cordis-abi | 2285 / 2700 |
| cordis-dev | 241 / 283 |
| cordis-host | 2345 / 2602 |
| cordis-process | 374 / 410 |
| cordis-runtime | 4304 / 4729 |
| cordis-sdk | 693 / 800 |
| cordis-sdk-macros | 1179 / 1280 |
| cordis-supervisor | 5463 / 5876 |
| cordis-testkit | 161 / 181 |
| cordis-types | 169 / 193 |
| cordis-up | 1613 / 1752 |
| 六个核心 crate 合计 | 16153 / 18072 |

M12 复核按用户要求从 6b4687f 导出隔离源码，修正测试与脚手架的构建目录后，以旁侧 `CARGO_TARGET_DIR` 执行全工作区测试，全部通过。命令为 `PATH=/opt/homebrew/opt/rustup/bin:$PATH CARGO_TARGET_DIR=/Users/gordongauerk/Projects/Rust-cordis/target/m12-review/build cargo test --workspace`，工作目录为 target/m12-review/source，输出保存在 target/m12-review/workspace-external-target.log。隔离目录初次命令的 PATH 缺少 Rustup 入口，在实际编译前退出 127；加入已安装工具路径后执行完成。Host 测试工具使用显式目标目录，生成项目使用 `cargo build --target-dir target`，相关修正随 M13 提交。M12 表中的 dev/runtime 数值已对 6b4687f 的源码重新计量。

### M13 契约验证与原生准入

- commit：本节所在的 `M13: 实现独立契约验证与原生准入门` 提交。
- 交付：cordis-contract、kv-contract、独立核心的两阶段契约会话、cordis-dev verify、共享结果/印记 DTO、宿主 SW-2 验证门及目录级单调副本序号。
- Y-1–Y-2：`cargo fmt --all --check` 与 `cargo clippy --workspace --all-targets -- -D warnings` 通过，完整 clippy 4.41 秒。Host 的缓存印记发布按步骤拆分后，Host 定向 clippy 通过，1.13 秒。
- Y-3：280 条运行测试与 2 条文档测试通过。完整工作区执行复用先前成功的两条真实 verify 流程，日志 target/m13-workspace-tests.log；这两条在 target/m13-verify-tests.log 中通过，27.53 秒。新增运行时准入场景通过，10.23 秒。`cargo build --workspace` 通过，4.92 秒。
- 真实契约：kv-mem 与 kv-file 各通过 8 条行为断言，阶段 0 执行读写、覆盖、排序和 flush，停止套件后导出并重启被测实例，阶段 1 检查继承值。违约真实库产生 7 条失败，进程终止库由独立核心报告 early EOF；两者 exit 1，成功库保存真实副本与印记。候选测得 RM-5 为 2723/1734 毫秒，正式自动化流程保留逐条输出。
- 真实准入：require_verified 模式下，缺印记的真实新库被拒绝；同字节旧 installed 引用取得真实印记后可以准入。用真实 kv-broken 字节覆盖候选并保留原印记时，SW-2 拒绝替换，原实例纪元与 KV 值 `[9,7,5]` 保持。已验证 kv-file 替换以 retired_image_limit=0 触发有状态宿主重启，缓存路径的印记和文件数据均验证通过。用例为 `gate::verified_runtime_install_replace_and_threshold_restart_use_real_receipts`。
- 副本序号：Host 以目录 File::lock 保护最大序号扫描与预留，两个独立宿主共享带旧格式序号 7 的目录时分配 8、9。开发工具的会话与验证副本也采用同一分配规则，已有高序号且低编号空缺的实际 verify 生成会话 8 与 kv-8；成功证据在 target/m13-dev-sequences.log。
- Y-4：M6 的 177 条矩阵校验通过，处置保持 8 条移植、166 条改写、3 条不适用。
- Y-5：工作区 unsafe 源码审计通过；核心产品依赖树在 target/m13-core-dependencies.txt，开发工具依赖树在 target/m13-dev-dependencies.txt。验证操作通过独立进程执行，开发工具使用数据与进程接口。
- Y-6–Y-7：本阶段 Miri 的 13 条边界用例通过，3.41 秒，布局比较通过，0.94 秒；原生布局比较通过，ABI 保持 v1.0。
- Y-8：下表沿 cloc 2.10 的 src 代码行 / 总行口径。runtime 的契约会话增加后为 4726 代码行，高于 4500 目标、低于 ADR-0034 的 5500 上限；其余 crate 保持各自上限。
- Y-9：README 验证命令、DOC-11 准入与缓存印记、DOC-13 的后续职责规划、M12 外置构建目录复核及代码行纠正已同步；解释页更新 M13 完成/M14 进行。
- Y-10：提交主题 `M13: 实现独立契约验证与原生准入门`，推送至 origin/main。
- 后续：M14 已准备真实 cordis 子进程的 50 次替换与 8 线程调用、直接调用基准、默认 Git 脚手架与 launchd 恢复。8 个独立库和 100 实例已取得预验证，启动 6139 毫秒；修改 load-one 后仅该插件重编译，记录在 target/acceptance-evidence 与 target/m14-ac1.log。

| crate | 代码行 / 总行 |
|---|---:|
| cordis | 4230 / 5112 |
| cordis-abi | 2285 / 2700 |
| cordis-contract | 31 / 47 |
| cordis-dev | 678 / 757 |
| cordis-host | 2470 / 2743 |
| cordis-process | 374 / 410 |
| cordis-runtime | 4726 / 5193 |
| cordis-sdk | 693 / 800 |
| cordis-sdk-macros | 1179 / 1280 |
| cordis-supervisor | 5463 / 5876 |
| cordis-testkit | 161 / 181 |
| cordis-types | 203 / 237 |
| cordis-up | 1613 / 1752 |
| 六个核心 crate 合计 | 16609 / 18580 |

### M14 系统验收与可替换性测量

- commit：本节所在的 `M14: 完成系统验收与可替换性测量` 提交。
- 交付：AC-1–AC-9、AU-1/2/8–11/16/18、RM-1–6/9/10 的证据与正式测试；README 默认 Git SDK 创建流程；替换报告计时与 Runtime 1.3。场景、计量口径和原始结果统一见 [M14 运行时验收与测量](measurements/m14-acceptance.md)。
- Y-1–Y-2：`cargo fmt --all --check` 与 `cargo clippy --workspace --all-targets -- -D warnings` 通过，clippy 3.91 秒。监管器随后共用四处报告发布，受影响文件格式检查及 supervisor 全 targets clippy 通过，后者 0.56 秒，见 target/m14-group-report-clippy-final.log；压力测试步骤提取后的目标 clippy 通过，1.00 秒。
- Y-3：288 条运行测试与 2 条文档测试通过。工作区执行通过 280 条运行测试和 2 条文档测试，复用本阶段已成功的 8 条定向验收，命令见下；完整日志 target/m14-workspace-tests.log。监管器报告发布重构后，`cargo test -p cordis-supervisor --test manager replacement_` 的 `failed_group_replacement_rolls_back_state`、`accepted_replacement_survives_waiter_cancellation` 两条通过，0.89 秒，见 target/m14-group-report-tests.log。`cargo build --workspace` 通过，4.64 秒；监管器重构后 `cargo build -p cordis-runtime` 通过，2.08 秒，见 target/m14-runtime-rebuild.log。
- Y-4：M6 一致性矩阵通过，177 条仍为 8 条移植、166 条改写、3 条不适用，见 target/m14-conformance.log。
- Y-5：工作区中的 `unsafe_is_localized_and_documents_its_contract` 通过，0.57 秒；`tools/unsafe_audit.py` 调用同一测试，复用该结果。核心产品依赖声明保持 M13 的 Q0 分离，见 target/m13-core-dependencies.txt。
- Y-6–Y-7：`MIRI_SYSROOT="$PWD/target/miri-sysroot" TMPDIR="$PWD/target/tmp" cargo +nightly miri test -p cordis-abi` 通过，13 条边界用例 3.41 秒、布局比较 0.94 秒；原生布局比较同样通过。工具链沿用环境表，日志 target/m14-miri.log。
- Y-8：下表按 cloc 2.10 的 src 代码行 / 总行口径。supervisor 的报告发布共用实现后为 5500 代码行，达到 ADR-0030 上限；目标 4500。runtime 为 4727 代码行，高于目标 4500、低于上限 5500；其余 crate 保持各自上限。六核心合计 16649 代码行 / 18625 总行。
- Y-9：README、DOC-11、ADR-0032/0035、测量原始 JSON、直接调用复现工程与解释页按实际行为更新。解释页 1440 桌面与 390 移动进度区截图已查看，路径为 target/explainer-m14-desktop.png、target/explainer-m14-mobile.png，控制台 0 错误；浏览器与 HTTP 服务已退出。模板命名规则在生成文件前检查，`hello_world` 与 `MyPlugin` 的真实失败回归通过，项目配置与目录保持；日志 target/m14-name-before.log、target/m14-name-after.log。
- Y-10：提交主题 `M14: 完成系统验收与可替换性测量`，推送目标 origin/main。
- 固定压力：实际 cordis 子进程中的 8 个线程完成 50 次替换，每轮读取状态 137；第 21、42 次触发有状态宿主重启，idle 纪元只在这两次变化。RM-3 中位数 3 / 最大值 4，RM-4 中位数 241 ms / 最大值 661 ms；完整 RPC 中位数 260.5165 ms / 最大值 682.222 ms。总 RSS 增量 118.4 KiB/次，核心 27.84 KiB/次；两次宿主 RSS 分别由 13664→5616 KiB、13712→5632 KiB。日志 target/m14-ac9.log；50 轮原始样本随仓库保存。
- 直接调用：M9 基线 dc93da1 与当前 consumer 同为 Rust 1.85 release，同一 provider；固定 9 轮、每轮 10000 次预热和 100000 次完整调用。中位数 19.66792 ns / 18.62083 ns，中位数比 0.946762、两组最大值比 0.721550，达到预定 1.05/1.15 标准。逐轮下降趋势及全部样本同时保留；测量按既定轮次结束。正式复现 driver 编译通过，8.03 秒。
- 真实部署：8 库 100 实例启动 6139 ms；修改 load-one 后实际重编译 1 个 crate。默认 Git SDK 的空目录创建、插件构建与 Active 流程通过。launchd 恢复耗时 9938 ms，旧 Host 退出，状态 73 覆盖配置初值 29，临时 job 已卸载。
- KV 链路：kv-mem → kv-file → kv-mem 0.2 后计数达到 29、50；失败版本自动回滚，kv@2 候选在 SW-4 拒绝；1.85 counter 继续到 107，SIGTERM 保存 109，重启达到 113。两个真实文件 KV 的隔离与三种提供方位置的 Scoped metadata 均通过。
- 后续：M15 候选手册、接口/组件/提供方/契约模板的 SK-3 编译已通过，1 条 / 7.72 秒；正式迁入后与 cordis-dev 共用模板，并由全新会话分别执行 SK-1、SK-2。后续持续推进 M16–M21，保留构建缓存。

工作区退出检查通过 `--skip` 复用本阶段成功事实：AC-1、README 和 AU-11 的日志为 target/m14-ac1.log、target/m14-readme-recovery.log；AC-9 为 target/m14-ac9.log；KV 迁移为 target/m14-kv-migration-observation-fixed.log；两条隔离用例为 target/m14-kv-isolation.log；命名回归为 target/m14-name-after.log。

```bash
cargo test --workspace -- \
  --skip eight_independent_libraries_and_one_hundred_instances \
  --skip readme_first_plugin_builds_from_pinned_git_sdk \
  --skip launchd_restores_core_and_checkpoint_after_core_exit \
  --skip eight_threads_survive_fifty_native_replacements_with_stateful_restarts \
  --skip continuous_counter_migrates_rolls_back_rejects_and_restores_across_compilers \
  --skip real_file_kv_instances_keep_tenant_data_separate \
  --skip kv_scoped_metadata_reaches_same_cross_and_process_providers \
  --skip rejected_plugin_name_keeps_project_unchanged
```

| crate | 代码行 / 总行 |
|---|---:|
| cordis | 4230 / 5112 |
| cordis-abi | 2285 / 2700 |
| cordis-contract | 31 / 47 |
| cordis-dev | 703 / 784 |
| cordis-host | 2470 / 2743 |
| cordis-process | 374 / 410 |
| cordis-runtime | 4727 / 5194 |
| cordis-sdk | 693 / 800 |
| cordis-sdk-macros | 1179 / 1280 |
| cordis-supervisor | 5500 / 5918 |
| cordis-testkit | 161 / 181 |
| cordis-types | 205 / 239 |
| cordis-up | 1613 / 1752 |
| 六个核心 crate 合计 | 16649 / 18625 |

### M15 插件编写手册、共享模板与服务安装

- 交付：`skills/cordis-plugin/SKILL.md`、三份按需阅读的参考和八份接口/组件/提供方/契约模板；`cordis-dev` 通过 `include_str!` 直接使用同一组件源码与 manifest。手册说明 PR-1–PR-8、服务和事件、Scoped/隔离、冻结状态与迁移、真实契约和部署。
- SK-1 首次热替换观测：独立 `codex exec` 会话从空目录编写 journal-counter，注入 kv@1 并导出 journal-counter.snapshot。0.1.0 最后计数与导出值均为 189；0.2.0 继承 189 后递增到 190–193，独立 KV 消费者读到对应值。核心 PID 19018、宿主 PID 19031 保持，批次 succeeded；有序退出返回 0，进程清理记录为空。会话 thread `01a1d172-1cdb-7d72-9a83-84d8427612f3`，完整结果 target/skill-eval/sk1-final.txt，原始目录 target/skill-eval/sk1/journal-demo/evidence/run-02，状态记录见 [SK-1 观测](measurements/m15-skill-state.json)。独立项目保留全局规则，按独立 Git 根加载作者 skill 与必要原始材料。
- SK-2：独立会话 `01a1d17c-c1fc-7353-a4fe-1d58d652f589` 使用有序 Vec 与 binary_search 编写 kv-vector，共用原 kv-api 的 Snapshot。实际 kv-contract 的 8 条断言全部通过，覆盖读写、覆盖、键排序、flush 与跨阶段状态继承，RM-5 为 1985 ms。报告为 target/skill-eval/sk2/kv-vector/evidence/verify-report.txt，真实印记为 artifacts/libkv_vector.dylib.verified.json。
- SK-3：`cargo test -p cordis-runtime --test skill_examples` 通过，1 条 / 9.11 秒；组件、接口、提供方、契约套件全部使用当前 SDK 编译，见 target/m15-skill-examples.log。skill-creator 的 quick_validate 已通过，见 target/skill-m15-validation/quick-validate.log；后续仅补充资源参考正文，frontmatter 与模板保持。
- Y-1–Y-2：工作区格式检查通过，服务修改后复用 target/m15-format-final.log。Clippy 按已成功目标分段覆盖全部 workspace/all-targets：dev/runtime 1.04 秒，其余包 1.67 秒；服务修改后 runtime 定向通过，3.51 秒。日志为 target/m15-clippy.log、target/m15-clippy-remaining.log、target/m15-service-clippy.log。
- Y-3：291 条运行测试与 2 条文档测试通过。工作区执行 281 条运行测试和 2 条文档测试，复用当前 SK-3 与 M14 七条专项场景；后续新增服务单元测试 2 条通过，AU-11 按新节流配置定向通过并取代该项旧观测。工作区命令沿 M14 的跳过范围，将命名回归换为 `shared_skill_templates_build_with_current_sdk`；日志 target/m15-workspace-tests.log、target/m15-service-unit.log、target/m15-au11.log。`cargo build -p cordis-dev` 通过，0.87 秒；真实 new/new-plugin 转交、构建与运行流程通过，27.15 秒，AU-11 自行构建当前核心与宿主。
- 服务安装：实现按 DOC-15 §5.1 生成持久服务位置；macOS 使用标准 LaunchAgents/LaunchDaemons，systemd 使用用户单元目录并 enable --now。`--output` 和 `--dry-run` 生成供检查的配置；uninstall 停止服务并删除对应配置。默认目录与缺席服务保留相邻文件的行为由两条单元测试检查。实际 launchd 验收在工作区内生成配置并显式 bootstrap，ThrottleInterval 生效后 RM-10 为 1038 ms，检查点 73 覆盖新配置 29，旧 Host 已退出、临时服务已回收，结果见 [M15 核心恢复](measurements/m15-core-recovery.json)。
- 代码量：runtime 4898 代码行 / 5392 总行，dev 692 / 770；六核心合计 16820 / 18823。统计来自 target/m15-source-lines.log，其余核心实现沿用 M14。supervisor 的后续增量依 DOC-13 先提取 cordis-routing 并写 ADR。
- 解释页：M15 进度区与独立 HTML 已同步；1440 桌面和 390 移动画面已查看，移动表格横向滚动可读。截图为 target/explainer-m15-desktop.png、target/explainer-m15-mobile.png、target/explainer-m15-mobile-details.png；控制台 0 错误，页面与本次 HTTP 服务已退出。
- 用户安装：按用户授权创建 `/Users/gordongauerk/.codex/skills/cordis-plugin`，指向 `/Users/gordongauerk/Projects/Rust-cordis/skills/cordis-plugin`。创建前 `ls -ld` 返回 No such file or directory；随后 `ln -s` 成功。`ls -l` 实际输出为 `lrwxr-xr-x@ 1 gordongauerk staff 61 Nov 12 15:44 /Users/gordongauerk/.codex/skills/cordis-plugin -> /Users/gordongauerk/Projects/Rust-cordis/skills/cordis-plugin`（终端本地日期）。安装范围为 Codex 用户目录。
- 持续任务修订：SK-1 生成的计数循环通过 `frozen.check()?` 传播暂停错误，并在 KV 写入前提交本地计数。原库在独立部署中按 checkpoint_ms=1、counter_interval_ms=250 固定观察 30 秒，139 次 KV 读取均为 100、1840 次导出均为 100、递增记录为 0，三个实例均 Active。动态证据确认停滞，code 4 的具体退出路径由 SDK/Host/生成插件的源码调用链确定；本次原观察者记录 KV 读值，后台错误事件记录缺项。证据为 target/skill-eval/sk1-checkpoint/evidence/{observations,interpretation,cleanup}.json；退出 0，两个进程均已回收。资源指南已补充冻结期等待、跨服务在途操作和提交语义。
- SK-1b：全新线程 `01a1d1bb-202b-7292-99d8-f5c1dbf70cd7` 使用修订手册，CLI 退出 0。独立观察者处于另一个宿主；固定 30 秒、checkpoint_ms=1、counter_interval_ms=250，118 条有效读取由 4 增至 108，六个 5 秒分段均增长。旧版状态 58，新版继承 58 后读到 59；有序退出时本地状态与 KV 均为 109。成功导出累计 1735 次，KV 冻结等待 4 次后持续增长；替换 succeeded，暂停 124 ms，全部进程回收。原始 JSON 保存为 [检查点与热替换观测](measurements/m15-skill-checkpoints.json)，命令及原始记录位于 target/skill-eval/sk1b/counter-demo/evidence，完整回复为 target/skill-eval/sk1b-final.txt。本次重验针对已确认的后台停滞问题，修订后达到持续运行与状态继承要求。
- Y-9–Y-10：进度、参考指南与解释页已更新；仓库提交 `ce7c89e`，主题为 `M15: 交付插件编写手册与持久服务安装`，已推送 origin/main。用户环境安装按上条授权结果完成。
- 后续：M16 的正式实现与验收见下一节。

### M16 发布打包、页面入口与窗口进程

- 正式实现：cordis-dev 的 pack、共享 tooling/receipts；Runtime 的 Bundle 入口与 Locations；Host 的多验证根；plugins/web-api、http、window-api、window。设计依据为 ADR-0036–0038。运行状态、安装副本、宿主缓存均支持包外目录；窗口异步修改在冻结期排队至 thaw。config.rs、manifest.rs、storage.rs 的解析源码保持既有版本，M12 对应 fuzz 结果继续有效。
- 构建：正式 Runtime/Host 构建通过，7.48 秒；dev/window/http 构建通过，20.36 秒，日志 target/m16-new-crates-build.log。窗口过程测试自行构建当前核心、开发工具、宿主、HTTP、窗口、KV 和真实 probe。
- 发布验收：`cargo test -p cordis-dev --test pack -- --nocapture` 五条通过，44.69 秒，见 target/m16-pack-tests.log。覆盖离开构建目录的真实原生 tar 包运行、Rust 进程使用 runtimes 命令改写、显式静态资源、签名后真实契约重验和移动，以及 app 包外写入与多验证根。场景目录为 target/pack-tests/{native,runtime,assets,signed,app}-48256；macOS 签名检查均沿实际产物执行。
- HTTP：正式 workspace 的真实请求用例通过，1 条 / 0.08 秒，覆盖默认页面、自定义入口与静态资源、会话信息、停用后的连接和端口释放。Web.entry 返回实际监听地址及本次激活令牌，M20 继续资源登记与 WebSocket 通道。
- 窗口过程验收：`cargo test -p cordis-runtime --test window -- --nocapture` 一条通过，29.76 秒，外层编译 2.43 秒。周期检查点为 250 ms，四实例 Active 后只注入一次 SIGKILL；窗口 PID 52725→52735，875 ms 恢复，重启次数 1，核心 PID 52717 保持。HTTP/KV context 保持 5/7，window 为 9→14，probe 为 11→16。probe 的真实 apply 标记 1→2，两次窗口编号均为 6，called 与 closed 按同一标记配对。SIGTERM 后退出 0、残留进程 0，`codesign --verify --strict` 通过。结果保存为 [M16 窗口过程恢复](measurements/m16-window-recovery.json)，原始证据为 target/window-tests/recovery-51816/{result,before-crash,after-crash}.json 与 core.log。M16 分段退出检查中的同一目标通过，7.23 秒，见 target/m16-runtime-remaining.log。
- 状态发布验收：migration 等待本次 KV rolled_back 报告与 counter Active，同文件跨编译器替换等待 Rust 1.85 活跃库；stress 按候选库编号区别前后两轮报告。隔离当前源码固定连续执行指定 migration 测试 20 次，20/20 通过，总耗时 234.848 秒，测试主体累计 214.25 秒。首轮命令 44.132 秒，后续测试主体 8.62–10.96 秒；结束时残留进程 0。提交证据包括[固定条件](measurements/m16-migration-plan.json)、[逐轮结果](measurements/m16-migration-rounds.json)和[最终结果](measurements/m16-migration-summary.json)，保留原始命令、源码范围、日志和场景路径。原始文件位于 target/m16-migration-review/evidence/{rounds,plan,summary}.json，日志为 round-01.log 至 round-20.log；完整进度素材为该目录上一级的 progress.txt。隔离构建缓存已清理，源码与验收产物保留。
- Y-1–Y-2：工作区 fmt 与 clippy/all-targets 通过；clippy 15.91 秒。日志 target/m16-fmt.log、target/m16-clippy.log。fixture 的 JSON 发布修复与 Rust 1.96 条件语法调整后，定向 clippy 通过，0.14 秒，日志 target/m16-fixture-clippy-fixed.log；该次独立构建目录已清理。
- Y-3：299 条运行测试与 2 条文档测试形成完整 workspace 覆盖。首次执行中 267 条通过，隔离域用例读取写入中 JSON 遇到 EOF；fixture 的 store/reader 观察值随后与既有 admin 回复共用先写 next、再 rename 的 publish 实现，该用例定向通过，1 条 / 1.61 秒，其余已成功结果复用。随后按执行中断位置完成 Runtime 的 skill_examples/stress/watch/window 共 4 条、其余包 27 条与文档测试 2 条。日志分别为 target/m16-workspace-tests.log、target/m16-realms-fixed.log、target/m16-runtime-remaining.log、target/m16-remaining-tests.log、target/m16-doctests.log。固定 20 次迁移验收作为重复运行样本单列。
- Y-4：177 条上游矩阵通过，8 条移植、166 条改写、3 条不适用，见 target/m16-conformance.log。
- Y-5：正式 workspace 的 `unsafe_is_localized_and_documents_its_contract` 源码审计通过，0.62 秒，结果见 target/m16-workspace-tests.log。核心依赖树保存在 target/m16-core-dependencies.txt；对 ABI、SDK、contract、GUI、监视、WIT 与动态库装载依赖的查询结果为空，符合 Q0 的进程职责划分。
- Y-6–Y-7：本阶段 Miri 的 13 条 ABI 边界用例通过，3.41 秒；布局比较通过，0.94 秒。原生布局用例在 workspace 执行中通过。Miri 日志为 target/m16-miri.log，ABI 保持 v1.0。
- Y-8：cloc 2.10 按各成员 src 中的 Rust 文件统计，参数为 `--skip-uniqueness --sdir=target/tmp --include-lang=Rust --by-file --json`，原始结果 target/m16-source-lines.json。各 crate 均满足 DOC-13 上限；下表列代码行 / 总行。六个核心 crate 为 cordis、cordis-up、cordis-supervisor、cordis-runtime、cordis-types、cordis-process，合计 16972 / 18991。
- Y-9：DOC-13 的 crate、依赖、包外目录与接口，DOC-15 的 M16/M20 职责、窗口和打包规则，以及 README 发布说明已同步。解释页 M16/M17 进度区及独立 HTML 已更新；实际查看 1440 桌面与 390 移动画面，截图为 target/explainer-m16-desktop.png、target/explainer-m16-mobile.png；移动表格可横向阅读，控制台 0 错误，页面与本次 HTTP 服务已退出。
- Y-10：提交 `3710989`，主题 `M16: 实现发布打包与独立窗口进程`，已推送 origin/main。
- AU-17 授权界面验收：按用户授权运行 target/window-m16/inspect_window.swift，对本次窗口 PID 读取系统应用信息、截图和发送 Cmd-Q。窗口 PID 64939→65170，982 ms 恢复，核心 PID 64904 保持，HTTP/KV context 保持 5/7。[恢复前应用信息](measurements/m16-window-app-before.json)和[恢复后应用信息](measurements/m16-window-app-after.json)的系统列表各记录一个 Regular 进程。截图命令返回 1，原因是屏幕录制权限等待允许；Cmd-Q 命令返回 1，原因是辅助功能权限等待允许，按键发送停在权限请求。真实画面、Dock 外观及快捷键退出结果保留缺项。随后以 SIGTERM 有序退出，返回 0，5 个进程均退出，应用签名有效。[授权执行结果](measurements/m16-window-authorized.json)保留本次实际观测，原始信息、错误输出和结果位于 target/window-m16/authorized-{before,after,result}.json、authorized-capture.stderr.log 与 authorized-quit.stderr.log。
- AU-17 用户终端验收：2026-11-12 18:17（America/Los_Angeles），用户在 iTerm2 授予系统权限后执行 `target/window-m16/au17-run.sh`，结果 passed。四实例 Active，窗口 SIGKILL 后 802 ms 恢复，HTTP/store context 保持 5/7，window 9→14、probe 11→16；恢复前后各一个 Regular 应用。截图与 Cmd-Q 退出码均为 0，核心有序退出 0，window/store 检查点保存，残留与强制终止进程数均为 0，签名有效。完整结果为 [AU-17 用户终端验收](measurements/m16-au17-user.json)；原始目录 `target/window-m16/au17-user-runs/20261113T021701.346712Z-97183`。已查看该目录的 `window-restored.png`，窗口标题与“运行中”页面清晰。辅助程序自测入口共用 Acceptance 的 finally 清理，支持 `--inspect-only`。
- 阶段状态：M16 实现、阶段检查与 AU-17 界面验收通过。M15 的 Codex 用户 skill 链接已按授权安装，路径与创建结果见上一节。
- 后续：继续 M17 接口描述语言。WIT 候选位于 target/idl-m17，宏拆分候选位于 target/macros-m17；其行为在正式接入时按实际差异验证。正式构建缓存继续供后续里程碑使用。

Y-3 的完整执行与按中断位置继续的命令为：

```bash
cargo test --workspace
cargo test -p cordis-runtime --test runtime \
  deployment_realms_resolve_separate_providers -- --exact --nocapture
cargo test --workspace --tests \
  --exclude console-log --exclude cordis --exclude cordis-abi \
  --exclude cordis-contract --exclude cordis-dev --exclude cordis-host \
  --exclude cordis-http --exclude cordis-process --exclude cordis-runtime
cargo test -p cordis-runtime \
  --test skill_examples --test stress --test watch --test window
cargo test --workspace --doc
```

| crate | 代码行 / 总行 |
|---|---:|
| cordis | 4230 / 5112 |
| cordis-testkit | 161 / 181 |
| cordis-abi | 2285 / 2700 |
| cordis-sdk | 693 / 800 |
| cordis-sdk-macros | 1179 / 1280 |
| cordis-up | 1613 / 1752 |
| cordis-supervisor | 5500 / 5918 |
| cordis-types | 205 / 239 |
| cordis-process | 374 / 410 |
| cordis-host | 2491 / 2766 |
| cordis-dev | 1902 / 2071 |
| cordis-runtime | 5050 / 5560 |
| cordis-contract | 31 / 47 |
| kv-api | 17 / 27 |
| kv-mem | 73 / 84 |
| counter | 81 / 89 |
| cordis-watch | 579 / 623 |
| console-log | 465 / 515 |
| kv-file | 130 / 143 |
| kv-contract | 57 / 67 |
| web-api | 12 / 18 |
| cordis-http | 358 / 399 |
| window-api | 11 / 17 |
| cordis-window | 943 / 1020 |
| 六个核心 crate 合计 | 16972 / 18991 |

| 超过目标的 crate | 当前职责与上限 |
|---|---|
| cordis-up | RPC 引用所有权、背压与取消共用协议实现；1613 高于目标 1500，上限 2500 |
| cordis-types | 公开替换报告和批次结果的数据模型；205 高于目标 200，上限 400 |
| cordis-supervisor | 隔离、组合、原生替换、原快照回滚和有状态重启；5500 达到上限，后续增量先按 DOC-13 提取 cordis-routing |
| cordis-host | ABI 对象寿命、组内直连、故障回收和多根验证副本；2491 高于目标 2000，上限 3500 |
| cordis-runtime | 管理事务、契约会话、检查点、服务安装和 Bundle 目录解析；5050 高于目标 4500，上限 5500 |
| cordis-dev | 脚手架、真实验证、归档、运行时命令改写及签名后重验；1902 高于目标 1500，上限 2500 |

### M17 WIT 接口与共享编译目录

- 正式实现：`cordis-idl` 共享 WIT 解析模型，生成 Rust、Python、TypeScript 的提供方接口、数据类型与调用方代理；公开命令为 `cordis-dev idl gen`。`plugins/kv-api/kv.wit` 成为接口、Snapshot 与 Changed 的来源，生成 Rust 继续保持 kv@1 的方法顺序和状态表示。宏的 interface/plugin 入口已按职责拆分。ADR-0039 规定复合资源、借用资源、嵌套 option、future/stream 和脚本传输约定，ABI v1.0 布局与宿主槽位保持原值。
- Y-1：`cargo fmt --all --check` 通过，日志 `target/m17-fmt-final.log`。Y-2：工作区 clippy/all-targets 通过，10.93 秒，日志 `target/m17-clippy.log`；IDL 安装环境修正后的对应目标通过，3.42 秒，日志 `target/m17-idl-clippy.log`。`cargo build --workspace` 通过，18.73 秒，日志 `target/m17-build.log`。
- Y-3：隔离源码 `target/m17-review/source`，编译根 `CARGO_TARGET_DIR=$PWD/target/main-build`。完整范围为 302 条运行用例、2 条文档用例，均通过。首次工作区命令在 IDL 的 pip 安装步骤因本机 virtualenv 要求停止；该条 `pip --target` 命令局部设置 PIP_REQUIRE_VIRTUALENV=false 后，从受影响及后续 crate 接续，保留已成功的内核、ABI 等结果。日志为 `target/m17-workspace-tests.log`、`target/m17-workspace-remaining.log`、`target/m17-core-doctests.log`。最终 crate 与插件源码逐字节比较一致，独立 fixture 的 Cargo.lock 同步为实际测试解析的依赖。
- WIT 验收：公开 CLI 现场生成六份源码后执行真实 SDK、Python/TypeScript Client 与提供方的 CBOR 调用，并由 Rust 回读脚本输出，23.07 秒通过。每个 DOC-15 §7 映射行均有行为断言，涵盖接口版本、同步/异步、record、variant/enum、option/result、resource；还覆盖借用资源、异步取消、引用回收、future/stream 与嵌套 option 的独立编码期望。原始证据为 `target/m17-review/source/target/idl-acceptance/2357-1794536751886760000/`。SDK 两项新增资源编码/事件用例与既有回环测试均通过；生成 kv-api 同现有 KV 实现与真实 kv-contract 场景共同验收。
- 共享编译：cordis-testkit 的 build 模块统一编译根与 shared 路径，跨进程文件锁覆盖构建和实际产物复制。夹具、宿主驱动、迁移、发布、watch、测量与示例构建均复用 shared；`pack --target-dir` 允许指定复用目录。生成的用户项目继续使用自己的 target，并在测试通过后清理该编译缓存。隔离验收的编译根最终子目录为 debug、shared；Cargo 临时创建的空 tmp 目录已移除。证据放在源码副本的 target，与编译根分开。本阶段后续三个活动编译根为 `target/main-build`、`target/agent-shared`、`target/agent-cleanup`；历史 `target/debug` 保留，后续主会话使用 main-build。
- 共享路径实际验收：发布 5 条通过，68.18 秒；脚手架真实构建/启动通过，25.96 秒；原生宿主 14 个独立场景通过；配置与管理、迁移、watch、窗口、50 次替换压力及 AU-11 均执行。测试均在外置 CARGO_TARGET_DIR 下自行构建所需产物。新增的 cordis-testkit 依赖位于产品 crate 的开发依赖及独立测试驱动中。
- Y-4：177 条矩阵通过，8 条移植、166 条改写、3 条不适用，日志 `target/m17-conformance.log`。Y-5：unsafe 源码审计在工作区执行中通过，0.69 秒；核心产品依赖查询符合 Q0，完整树为 `target/m17-core-dependencies.txt`。
- Y-6：`MIRI_SYSROOT=$PWD/target/miri-sysroot TMPDIR=$PWD/target/tmp CARGO_TARGET_DIR=$PWD/target/main-build/shared cargo +nightly miri test -p cordis-abi` 通过，13 条边界行为 3.64 秒，布局 0.94 秒，日志 `target/m17-miri.log`。nightly 沿用环境表。Y-7：原生布局用例也在隔离工作区测试中通过。
- Y-8：下表来自 `conda run -n base python tools/source_lines.py`，使用 cloc 2.10、`--skip-uniqueness` 与 src 内 Rust 代码行口径，日志 `target/m17-source-lines.txt`。IDL 2324/2466，低于 ADR-0039 的 2500 目标；宏 1434/1566，复合资源适配使其高于 1200 目标，低于 2000 上限；其余超过目标的原因沿用 M16，全部保持各自上限。
- Y-9：DOC-12、DOC-13、DOC-15、README 与解释页已同步，AU-17 用户终端通过结果归档至 `docs/measurements/m16-au17-user.json`。实际查看解释页 1440 桌面和 390 移动画面，截图 `target/explainer-m17-desktop.png`、`target/explainer-m17-mobile.png`，进度文字可读，表格沿既有横向滚动布局，控制台 0 错误；页面及本次 HTTP 服务已退出。
- Y-10：提交主题 `M17: 实现 WIT 三语言绑定与共享编译目录`，推送目标 origin/main。
- 偏离与决定：ADR-0039 将解析与生成放入 cordis-idl 开发库，采用 scoped-tls 为 Serde 复合资源传递同步调用环境；生成 Rust 使用独立 syn 3/prettyplease，原生 SDK 宏保持 syn 2 和 Rust 1.85。依赖均位于相应开发工具或原生插件侧。

```bash
export CARGO_TARGET_DIR="$PWD/target/main-build"
export TMPDIR="$PWD/target/tmp"
cargo test --manifest-path target/m17-review/source/Cargo.toml --workspace
# IDL 安装命令修正后接续其余 crate。
cargo test --manifest-path target/m17-review/source/Cargo.toml --workspace \
  --exclude cordis --exclude cordis-abi --exclude cordis-contract \
  --exclude console-log --exclude counter
cargo test --manifest-path target/m17-review/source/Cargo.toml -p cordis --doc
```

| crate | 代码行 / 总行 |
|---|---:|
| cordis | 4230 / 5112 |
| cordis-testkit | 208 / 240 |
| cordis-abi | 2407 / 2846 |
| cordis-sdk | 786 / 910 |
| cordis-sdk-macros | 1434 / 1566 |
| cordis-idl | 2324 / 2466 |
| cordis-up | 1613 / 1752 |
| cordis-supervisor | 5500 / 5918 |
| cordis-types | 205 / 239 |
| cordis-process | 374 / 410 |
| cordis-host | 2491 / 2766 |
| cordis-dev | 1934 / 2112 |
| cordis-runtime | 5050 / 5560 |
| cordis-contract | 31 / 47 |
| kv-api | 23 / 37 |
| kv-mem | 73 / 84 |
| counter | 81 / 89 |
| cordis-watch | 579 / 623 |
| console-log | 465 / 515 |
| kv-file | 130 / 143 |
| kv-contract | 57 / 67 |
| web-api | 12 / 18 |
| cordis-http | 358 / 399 |
| window-api | 11 / 17 |
| cordis-window | 943 / 1020 |
| 六个核心 crate 合计 | 16972 / 18991 |

### M18 Python SDK、进程插件与跨插口迁移

- 正式实现：`sdks/python/cordis_sdk` 提供 Component、Context、Effect、Frozen、双向 UP/1、WIT 绑定适配、调用取消和资源引用。`plugins/kv-py`、`plugins/counter-py` 使用逻辑名称 kv、counter，与原生实现共享接口及状态格式。`Context.isolate`、`intercept`、`plugin` 与 scoped 提供方覆盖 DOC-15 §6.4；事件可携带独立持有的服务资源。第三方 CBOR 依赖为 cbor2 5.6.5，依据 ADR-0040，Python 文件从运行入口使用现有 Conda base。
- 脚手架：`cordis new-plugin <名称> --python` 生成当前 SDK、Python 入口、清单与 requirements，在部署目录建立源码链接；Cargo exclude 和 watch 的源目录监视随配置更新，toml_edit 保留原有格式与注释。依赖安装位置为插件的 .dependencies。实际新建项目中 Rust 成员继续构建，Python 插件进入 Active；修改源码后 watch 执行新版本。验收等待新代码写入的 stderr 标记与新 context，覆盖 status.json 的异步发布时间差。最新定向通过用时 30.66 秒，日志 `target/m18-python-visible-version.log`，原始场景 `target/python-sdk/scaffold-36902`。生成项目的 Cargo 缓存已按规则清理，源码、状态及日志保留。
- 跨插口修复：原实现按候选类型选择宿主内换库，使 process→native 返回 code 3。单项替换和 replace_batch 现以当前端与候选端共同决定策略：两端原生使用库事务，跨插口使用已有的分组事务。supervisor 的生命周期实现保持原值。100 ms 周期检查点下，原生→Python→失败候选回退→原生→批次回到原生的读值依次为 1、99、200、255、366；失败候选返回 1001、committed=false，批次 succeeded。
- 资源专项：根据已复现的二次转交错误，Service 使用生成 schema 解码参数、调用远程代理并重新编码结果，每条连接持有自己的出口引用。参数与结果继续携带资源的镜像调用通过；事件回调结束后保留的服务仍可调用。修复前证据位于 `target/python-m18/evidence/resources-transfer-before/failure.json`，正式结果归档在 [Python SDK 验收](measurements/m18-python-sdk.json)。
- Y-1：fmt 通过，`target/m18-fmt-final.log`。Y-2：工作区 clippy/all-targets 的其余目标通过；脚手架测试拆分后的一处多余借用已修正，dev 全目标通过（含构建锁等待共 18.91 秒），最终 Python 目标通过 0.31 秒。日志为 `target/m18-clippy.log`、`target/m18-dev-clippy.log`、`target/m18-python-clippy.log`。runtime 与 dev 的实际构建分别通过 9.20 秒、6.96 秒。
- Y-3：`TMPDIR=$PWD/target/tmp cargo test --workspace` 全部通过，共 304 条运行用例与 2 条文档用例，日志 `target/m18-workspace-tests.log`。Python 集成的六个场景涵盖 Rust↔Python 服务调用、8 项原生 kv-contract 断言、资源构造/借用/二次转交、事件资源、调用取消、超帧后的继续调用、私有域子组件及父清理、命名域、双向替换和批次替换。正式自构建入口为 `crates/cordis-dev/tests/python.rs` 与 `tests/python-sdk/run.py`，实际程序由公共 testkit 的 shared 构建及快照提供，原始场景为 `target/python-sdk/behavior-32011`。全部场景及迁移、契约结果已汇入 `docs/measurements/m18-python-sdk.json`。
- Y-4：177 条矩阵通过，8 条移植、166 条改写、3 条不适用，日志 `target/m18-conformance.log`。Y-5：工作区中的 unsafe 审计通过；新增 Python 与格式编辑依赖位于插件侧或开发工具侧，核心依赖边界沿 Q0 保持。
- Y-6：ABI 与 Rust 宏源码保持 M17，复用 M17 的 13 条 Miri 边界用例及布局比较结果，nightly 版本沿环境表。Y-7：本阶段工作区执行的原生布局比较通过，ABI 为 v1.0。
- Y-8：cloc 2.10 的完整 crate 统计保存在 `target/m18-source-lines.txt`。变化项和核心合计如下，其他 crate 数值与 M17 一致，全部符合 DOC-13 上限。Python SDK 为 838 代码行 / 987 总行，六个模块按解析、对象所有权、上下文和组件执行分工。
- Y-9：ADR-0040、DOC-13、DOC-15、Python SDK README、根 README 与解释页已同步。已实际查看 `target/explainer-m18-desktop.png` 和 `target/explainer-m18-mobile.png`，新进度行可读；移动表格沿已有横向滚动布局。控制台 0 错误，页面与 HTTP 服务已退出。
- Y-10：提交主题 `M18: 接入 Python SDK 与跨插口状态迁移`，推送目标 origin/main。
- 设计调整：按用户的成熟解析库规则采用 cbor2，并为 M19 规定 cbor-x；详情见 ADR-0040。WIT 的 async 元数据维持原生入口选择，Python 提供方等待实际返回的 awaitable。SDK 源码、依赖和脚手架均保存在工作区；保持两个 Astra/high 子代理，分别准备 M20 的 routing 提取与网页接入。

| crate 或模块 | 代码行 / 总行 |
|---|---:|
| cordis-dev | 2062 / 2256 |
| cordis-runtime | 5056 / 5566 |
| Python SDK | 838 / 987 |
| 六个核心 crate 合计 | 16978 / 18997 |

### M19 TypeScript SDK、脚手架与测试现场管理

- 正式实现：`sdks/typescript/cordis-sdk` 提供 Node UP/1、Component/Context、效应、冻结与继承、服务与事件资源、显式 close、调用取消和 §6.4 的组合能力。`plugins/kv-ts` 采用 kv@1.0、kv.snapshot@1，与原生及 Python 实现共享替换身份。WIT 的 u64/s64 在短整数 CBOR 表示下转换为 bigint；提供方按实际 Promise 等待返回值。依赖为 cbor-x 1.6.0，TypeScript 5.9.3 与 Node 类型声明归开发依赖，沿 ADR-0040。
- `new-plugin --ts`：模板将 SDK 源码放入 src/cordis_sdk，统一编译到 dist；npm 的 bin-links=false 使安装副本保持普通文件。kv-ts 的本地 SDK 安装使用 install-links=true。实际生成项目的 Rust 成员及 Node 插件共同运行，重编译后 watch 执行新模块的 stderr 标记。Python 与 TypeScript 脚手架共用登记、Cargo exclude、watch 配置与测试工具构建。
- Y-1：fmt 通过，`target/m19-component-fmt.log`。Y-2：完整工作区 clippy/all-targets 通过，13.38 秒，`target/m19-component-clippy.log`。TypeScript SDK 和 kv-ts 的 tsc 编译通过；正式 SDK 场景每次通过公开 IDL 命令生成绑定并自行构建工具和插件。
- Y-3：按用户要求连续两次 `cargo test --workspace`，每轮 309 条运行用例、2 条文档用例通过，耗时 277.188 秒、247.984 秒。20 类测试现场在两轮之间的文件数、字节数、子目录数增长均为 0，详细统计为 [测试现场验收](measurements/m19-test-cleanup.json)。日志为 `target/cleanup-verification-m19-lifecycle/round-1.log`、`round-2.log`；执行脚本 `target/verify_test_cleanup_m19.py` 固定两轮，失败即停止。
- SDK 实际验收：Rust↔TypeScript 字节调用、8 项原生 KV 契约、scoped 元数据、私有域子组件与父清理、命名域、资源借用/二次转交/事件导入、AbortSignal 取消、超帧后的继续调用、Rust→TypeScript→Rust 状态继承及失败回退均通过。第二轮实际结果汇入 [TypeScript SDK 验收](measurements/m19-typescript-sdk.json)，原始证据位于 `target/test-evidence/typescript-sdk-behavior-93487-0/typescript-sdk/behavior-93487`；脚手架证据位于 `target/test-evidence/typescript-scaffold-93487-1/typescript-sdk/scaffold-93487`。
- 测试现场管理：公共 TestGuard 在断言和进程退出完成后标记成功，迁移 JSON、日志和状态证据后删除现场；失败保留。SharedFixture 静态保存 Weak，FixtureLease 保有程序副本并绑定测试结果，最后一个使用者释放；此前失败会保留共享现场。3 项实际文件用例覆盖成功归档、错误返回保留及跨租约失败保留。窗口、压力、watch、宿主、验证、打包、脚手架、语言 SDK 与各文件单元测试均已接入。
- 共享目录修复：第一轮发现混合 cdylib/rlib 输出覆盖同名 rlib，形成 SDK trait 身份冲突。ADR-0042 将 KvStore、Counter 移至唯一的普通组件库，动态插件保留原名称的纯 cdylib 入口；SDK 测试和 kv-evolution 直接依赖组件库。保留现有缓存的迁移及 SDK 回环定向通过，随后两轮完整执行通过。原始失败日志位于 `target/cleanup-verification-m19/round-1.log`。
- 状态观察修正：DOC-11 §7 要求旧组件重新 apply，回退测试等待旧版本引用、rolled_back 报告和持续计数；context 编号记录实际重建结果。旧断言读到延迟快照，错误地要求编号保持，失败记录位于 `target/cleanup-verification-m19-component/round-1.log`，定向修正结果 `target/m19-rollback-published.log`。
- Y-4：177 条矩阵通过，8 条移植、166 条改写、3 条不适用，`target/m19-conformance.log`。Y-5：两轮中的 unsafe 审计通过，宏测试入口的符号规则按 ADR-0042 保持生产 ABI。核心产品依赖延续 Q0；组件库归插件侧，testkit 归测试依赖。
- Y-6：ABI 实现代码保持 M17，复用该阶段 13 条 Miri 边界用例与布局比较，nightly 沿环境表。Y-7：两轮原生布局比较通过，ABI v1.0 保持。宏在 cfg(test) 下保留 Rust 符号名，生产导出名称及函数签名保持原值。
- Y-8：cloc 2.10 统计各 src 的代码行/总行，完整结果为 `target/m19-source-lines.txt`；TypeScript SDK 为 1126/1222。变化项见下表，其他 crate 数值沿 M18，均符合 DOC-13 上限。
- Y-9：README 采用设计方重写版本，只更新 M19 路线状态与 TypeScript 行数；Python 快速开始依赖路径与脚手架一致。完整用法保留于 docs/guide.md，新增 TypeScript 节。解释页已同步，实际查看 `target/explainer-m19-desktop.png`、`target/explainer-m19-mobile.png`，进度文字可读、控制台 0 错误；页面及本次服务已退出。
- Y-10：提交主题 `M19: 接入 TypeScript SDK 并治理测试现场生命周期`，推送目标 origin/main。
- 清理：旧编译 profile 41G→38G；旧测试现场 39G→32G，补充现场 33G→32G，各次目录占用实测和执行范围见“决策与发现”。已交付 AU-17 应用、Miri、现有活动编译根保留。双轮沿用已安装工具链、Cargo/npm 缓存及从 pip 缓存取得的本地 cbor2 wheel，环境设置保存在 `target/offline-env.sh`。

| crate 或模块 | 代码行 / 总行 |
|---|---:|
| cordis-dev | 2141 / 2345 |
| cordis-sdk-macros | 1427 / 1558 |
| cordis-testkit | 468 / 532 |
| kv-mem | 6 / 9 |
| kv-mem-component | 67 / 80 |
| counter | 6 / 9 |
| counter-component | 75 / 85 |
| kv-file | 129 / 142 |
| kv-contract | 56 / 66 |
| cordis-watch | 578 / 622 |
| cordis-http | 361 / 402 |
| TypeScript SDK | 1126 / 1222 |
| 六个核心 crate 合计 | 16978 / 18997 |

### M20 网页宿主、页面隔离与全栈替换

- 交付：cordis-routing 提取对象出口、资源转交与有界元数据；cordis-channels 负责每页独立 UP/1 连接；cordis-web-deployment 管理资源 Owner、页面子实例与共同恢复。HTTP 追加版本资源、WebSocket、Origin/令牌检查及默认地址恢复；cordis-web 复用 TypeScript Runtime。ABI v1.0 的布局保持原值。
- 脚手架与示例：new-plugin --web 支持纯网页，以及 Rust/Python/TypeScript 后端组合；原生 build.sh 将动态库放入插件目录。examples/web 包含笔记、Rust KV 面板和 Python KV 面板。Node SDK 与网页宿主由同一 TypeScript 编译入口构建，HTTP 内嵌包携带实际 cbor-x 许可证。
- Y-1：cargo fmt --all --check 通过，target/m20-fmt-integrated.log；后续测试修正经同一 formatter 输出更新。
- Y-2：完整工作区 all-targets clippy -D warnings 通过，target/m20-clippy-ready.log，2.39 秒；共享安装函数和 HTTP 的受影响目标通过，target/m20-clippy-helper-http.log，11.43 秒；独立原生测试 driver 的 clippy 通过，target/m20-driver-clippy.log，5.59 秒。
- Y-3：工作区全部目标分段完成，共 323 条运行用例、2 条文档用例通过。初次命令为 cargo test --workspace；两条原生 Scoped 测试改为单独计量实例存在信号，私域业务服务仍精确增加 1，定向两条 3.75 秒通过。HTTP 默认页检查转为其公开挂载节点。继续执行余下目标时，pip 的 PIP_REQUIRE_VIRTUALENV=true 使 web 夹具初始化失败；设计方 7b5bdb65 的意见通过公共 testkit 安装函数收口，idl/python/web 共用。修复后原 8 条 web 测试在外层环境仍为 true 时全部通过，32.74 秒；受影响 IDL 与 Python 脚手架分别 7.80 / 32.77 秒通过。其他有效结果按范围复用，完整分段计数见 [工作区结果](measurements/m20-workspace-tests.json)。
- 浏览器：8 个固定场景通过，包括两页局部替换与回退、Rust→Python 全栈切换及坏前端恢复、混合批次提交/恢复/configure/remove、三个语言提供方逐项撤回后的 Pending 与恢复、页面命名域与关页隔离、HTTP 同 origin 重连、协议故障及生成模板的动态装配。[浏览器结果](measurements/m20-web-acceptance.json) 保存原始 case 路径。洪泛为 128 个真实调用，开始与峰值均为 64，额度错误为 code 8；故障页关闭后其调用归零，健康页继续工作。
- 界面：正式 49453 批次的桌面与移动状态图已查看，组件间距及移动按钮布局已修正；docs/design/flow 交付 9 个状态、5 条操作关系、12 张截图。解释页进度区的 1280 / 390 视口已查看。浏览器记录定位的默认 /favicon.ico 404 改为 204，真实 HTTP 请求用例 0.09 秒通过；自定义资源目录沿原路由提供其图标。
- Y-4：177 条矩阵通过，8 条移植、166 条改写、3 条不适用；target/m20-conformance.log。
- Y-5：unsafe 审计通过，target/m20-unsafe.log，并在工作区执行中再次通过。核心产品依赖树保持 Q0，target/m20-core-dependencies.txt；原生宿主的产品依赖树保持与内核分离，target/m20-host-dependencies.txt。
- Y-6：ABI 实现与宏生产模板保持既有语义，复用 M17 的 13 条 Miri 边界用例与布局比较；工具链沿环境表。Y-7：本阶段工作区的原生布局测试通过，ABI 为 v1.0。
- CP-9：up1_decoder 增加转发载荷及内层消息解析，固定 max_total_time=1800，实际 1801 秒、1,663,834 次输入、退出 0。被测 cordis-up/src 与正式源通过 diff -qr 完整字节比较，测试目标通过 cmp；[模糊测试记录](measurements/m20-up1-fuzz.json) 保存命令与结果，原日志 target/m20-fuzz/fuzz.log。
- Y-8：cloc 2.10 --skip-uniqueness 统计各 crate/src 的 Rust 代码，排除空行与注释；SDK 单独按各自 src 的语言规则统计。完整表见下方。supervisor + routing 的注释合计 227 行，提取前为 178 行。supervisor、runtime、host、UP/1 与宏超过目标的部分分别来自生命周期和事务、部署管理、ABI 适配、协议预算及跨语言接口生成，均处于 DOC-13 上限内。
- Y-9：DOC-11/13/15/16、ADR-0041/0044、README 三张表、guide、网页 SDK 文档、解释页与流程图已同步。ADR-0043 已分配给 M21 独立运行时打包候选，随 M21 提交。
- Y-10：提交主题 M20: 接入网页宿主、页面隔离与全栈替换，推送目标 origin/main。
- 决策与修正：ADR-0041 使用 Context 扩展与 (页面编号, 域名) 键隔离页面命名域，保留后端全局命名域；旧核心的同名服务重复注册证据与修复后的 0/0→1/0 证据分别在 routing/evidence/realm-provider-active、realm-controls。父级在 child 登记期间结束时等待其回收；稳定检查重新定位仍登记的 Instance；失败 Owner 在共同回退时按原快照重新激活。ADR-0044 为配置调和保留 Active/Pending 稳定语义，纯版本替换沿 SW-7。
- 接续：M21 的独立 Python/Node 签名包、六方向覆盖与三语言迁移、skill 章节候选已经准备；固定 RM-7/8/9 正按预定次数执行。正式整合从 target/phase3-acceptance/m21 的说明继续。

| crate 或 SDK | 代码行 / 总行 |
|---|---:|
| cordis | 4230 / 5112 |
| cordis-abi | 2407 / 2846 |
| cordis-sdk | 789 / 915 |
| cordis-sdk-macros | 1427 / 1558 |
| cordis-up | 1628 / 1773 |
| cordis-supervisor | 5480 / 5923 |
| cordis-routing | 580 / 660 |
| cordis-channels | 250 / 277 |
| cordis-web-deployment | 850 / 918 |
| cordis-runtime | 5313 / 5839 |
| cordis-host | 2719 / 3007 |
| cordis-dev | 2278 / 2498 |
| cordis-idl | 2324 / 2466 |
| cordis-process | 374 / 410 |
| cordis-types | 205 / 239 |
| cordis-testkit | 502 / 571 |
| cordis-contract | 31 / 47 |
| console-log | 465 / 515 |
| cordis-watch | 578 / 622 |
| cordis-http | 823 / 892 |
| cordis-window | 943 / 1020 |
| web-api / forward-api / window-api | 25/32；17/21；11/17 |
| kv-api / kv-contract / kv-file | 23/37；56/66；129/142 |
| kv-mem / kv-mem-component | 6/9；67/80 |
| counter / counter-component | 6/9；75/85 |
| Python SDK | 838 / 987 |
| TypeScript SDK / 网页宿主 | 1163/1261；206/231 |
| 核心九个 crate 合计 | 18910 / 21151 |

### M21 三语言验收、独立运行时发布与插件手册

- 交付：完整 Python/Node 运行时目录进入发布包，命令指向包内实际解释器入口；深层 Mach-O、程序及外层 Bundle 按顺序签名。Node 使用 JIT 权限，ad-hoc Python/Node 具有装入包内原生扩展所需的权限。职责与依据见 ADR-0043。
- AU-3–AU-6：Rust、Python、TypeScript 提供方与跨语言使用方同核运行；新增 Python→TypeScript、TypeScript→Python 真实字节服务调用，结合已有四方向覆盖六个方向。同一 Rust counter 持续写入时，提供方依次由 Rust、Python、TypeScript 接续，状态和计数保持。三种 KV 各通过原生 kv-contract 的 8 项断言。原始场景汇入 [跨语言调用](measurements/m21-language-cross.json)与[连续迁移](measurements/m21-language-migration.json)。
- AU-12：完整 Python 3.12.16 与 Node 22.23.5 的 ad-hoc 签名 tar 包，移动解开后实际执行 Python _cbor2/SSL、Node 原生 CBOR 扩展及动态代码；两种 KV 读写、签名校验与全部进程退出通过。[运行时发布结果](measurements/m21-script-runtimes.json)来自本次 workspace 的正式用例。冷缓存准备分支另实际下载 51,157,421 字节，解开后与已有来源逐文件字节及链接比较通过，重复输入随后释放。Developer ID 路径按同团队规则实现，本次真实签名身份为 ad-hoc。
- AU-1 稳定性修复：设计方在 M20 全量负载中观察到 segmentation_fault_isolates_group 一次失败。其原 200 ms invoke 截止时间会发 cancel，调度拥堵时故障调用可能在实际触发前被取消；改为沿既有 20 秒场景总预算，spin 仍执行 3 次 200 ms 连续超时。故障前订阅 InternalStatus 并读取 fiber.state()，等待连接 EOF、依赖 Pending 通知及 settled，再检查分组退役；两段 1 ms 轮询已移除。13:48:44–13:49:40 UTC 完整 native 固定连续五轮，每轮 14 条全部通过，五轮全程处于同一次 workspace 负载内。[稳定性记录](measurements/m21-native-stability.json)保存每轮命令、时间、退出码与交叠区间。
- Y-1：`cargo fmt --all --check` 通过，`target/m21-fmt.log`。
- Y-2：`cargo clippy --workspace --all-targets -- -D warnings` 通过，16.66 秒，`target/m21-clippy.log`；独立测量 driver 的 clippy 通过，5.26 秒，`target/m21-measurement-driver-clippy.log`。正式 probe 的 release 构建通过，4.21 秒，`target/m21-formal-probe-build.log`。
- Y-3：`cargo test --workspace` 一次完整通过，325 条运行用例与 2 条文档用例，共 327 条，460.772 秒，退出 0。运行区间为 2026-11-13 13:48:44.014–13:56:24.788 UTC；原日志在 `target/phase3-acceptance/m21/native-stability/evidence/20261113T134844Z/workspace.log`。完整发布包及语言夹具均由测试自动构建、安装依赖并运行，成功现场由 TestGuard 归档。
- Y-4：内核与用例矩阵保持 M20 的正式源，复用其 177 条通过结果：8 条移植、166 条改写、3 条不适用，`target/m20-conformance.log`。
- Y-5：本次 workspace 的 unsafe 审计通过。核心九个 crate 及其依赖声明保持 M20，复用 `target/m20-core-dependencies.txt` 的 Q0 结果与 `target/m20-host-dependencies.txt` 的宿主依赖结果。
- Y-6：ABI 实现、生产宏及布局文件保持 M17 的已验源，复用 13 条 Miri 边界用例及布局比较，nightly 版本见环境表。Y-7：本次 workspace 的 ABI 布局用例通过，冻结版本为 v1.0。
- Y-8：沿 `cloc 2.10 --skip-uniqueness` 分别统计 crate/src 与 SDK 源目录，报告代码行 / 总行；完整表见下方。cordis-dev 增至 2308 / 2532；supervisor 5480 / 5923、runtime 5313 / 5839，均处于 5500 代码行上限内；核心九个 crate 合计 18910 / 21151。超过目标的职责沿 M20 的生命周期、事务、部署、协议预算及语言接口生成说明。
- Y-9：AU-1–AU-18 覆盖与复用源码范围见[第三阶段验收](measurements/m21-acceptance.md)。README 的路线图、组成和实测表，guide、DOC-13/15、ADR-0043、skill 与解释页已同步。解释页受影响进度区的桌面与移动截图已查看，page_errors=[]；证据为 `target/m21-evidence/explainer/progress-desktop.png`、`progress-mobile.png` 和 `result.json`。
- Y-10：提交主题 `M21: 完成三语言验收与独立运行时发布`，推送目标 origin/main。
- skill：保留 Rust 接口、效应、资源、状态和契约指南，新增 Python、TypeScript、网页/全栈、打包及模板来源章节。模板复用 cordis-dev 维护源；skill-creator 的 quick_validate 通过，Rust 模板继续由 workspace 编译验证，脚本语言和网页模板由真实 SDK、脚手架及网页用例覆盖。Codex 用户 skill 链接继续指向仓库目录。
- 提交前清理：两个子代理结束后，进程及打开文件查询确认目录空闲，释放 target/agent-cleanup、target/agent-shared、target/debug/incremental；M20 及以前目录中逐个识别的 Mach-O、ELF、静态库副本和 window-m16 的 Swift 模块缓存共形成 612 项删除记录。`du -sh target` 为 60G→30G，目录占用减少 31,761,160 KiB；文件系统可用空间同期实测增加 31,625,617,408 字节。完整删除清单及前后数值为[清理记录](measurements/m21-cleanup.json)。日志、JSON、截图与语料原位保留，引用证据迁移项为 0；review-fix、review-m3、review-m4、Miri、claude-edits 及 M21 原始样本和暂停候选保持。
- AU-17 目录：`target/window-tests/recovery-51816` 按“用户 2026-11-13 授权删除”（6990ee77）整体移除，删除前确认该 app 与 cordis 相关进程均已退出。正式结果保存在 `docs/measurements/m16-au17-user.json`；window-m16 的 inspect-window、Swift/Python/zsh 脚本、用户结果与截图继续保留。后续执行构建时沿默认 target/debug 与 target/shared 重建所需产物。

RM-7 的原生调用方和提供方位于不同分组，计时覆盖请求、处理、复制及结果释放；每向固定 10000 次。RM-8 三个方向各执行一次，pause 沿 ADR-0035 从事务准备计至受影响实例全部 Active。环境为 Apple M4 Pro、14 个逻辑 CPU、48 GiB 内存、macOS 27.2 arm64、Rust 1.96.0 release，直接调用 consumer/provider 使用 Rust 1.85.0。完整原始 JSON 和构建日志见[性能记录](measurements/m21-performance/README.md)。

| 指标 | 中位数 / 99 分位，或替换停顿 |
|---|---:|
| RM-7 Rust→Rust（跨分组） | 124.833 / 237.750 µs |
| RM-7 Rust→Python | 116.875 / 146.125 µs |
| RM-7 Rust→TypeScript | 81.375 / 175.084 µs |
| RM-8 kv-mem→kv-file | 215 ms |
| RM-8 Python→TypeScript | 362 ms |
| RM-8 TypeScript→Python | 126 ms |

| RM-9 窗口 | 基线 / 当前中位数 | 中位数比 | 最大值比 | 原判据 |
|---|---:|---:|---:|---|
| 首轮，与设计方全量复核重叠 | 19.14959 / 20.62625 ns | 1.07711183 | 0.89729184 | 中位数比超过 1.05 |
| 各方停止构建及测试后的重测 | 20.22250 / 19.36792 ns | 0.95774113 | 0.90793525 | 满足 1.05 / 1.15 |

RM-9 两次均为每实现固定 9 轮、每轮预热 10000 次、计量 100000 次。首轮于 13:06 UTC 结束，与设计方持续至 13:10 UTC 的全量复核重叠；按 45fa107f 的新负载证据协调停载，原正式实现于 13:39:54 UTC 重测一次，原快照、轮次及判据保持。两次前 1–4 轮均偏高，两个窗口的中位数比相差约 12%，大于 5% 判据宽度，记录为短预热、短测量窗口的已知局限。两轮原始样本完整保留；预热长度和交替采样方法列为后续独立研究。按 745fc7dd，ADR-0045 静态分派候选保存在 `target/phase3-acceptance/m21/rm9-candidate` 与 `rm9-analysis`，状态为暂停，正式 SDK 与 Service 沿既有实现。

| crate 或 SDK | 代码行 / 总行 |
|---|---:|
| cordis | 4230 / 5112 |
| cordis-abi | 2407 / 2846 |
| cordis-sdk | 789 / 915 |
| cordis-sdk-macros | 1427 / 1558 |
| cordis-up | 1628 / 1773 |
| cordis-supervisor | 5480 / 5923 |
| cordis-routing | 580 / 660 |
| cordis-channels | 250 / 277 |
| cordis-web-deployment | 850 / 918 |
| cordis-runtime | 5313 / 5839 |
| cordis-host | 2719 / 3007 |
| cordis-dev | 2308 / 2532 |
| cordis-idl | 2324 / 2466 |
| cordis-process | 374 / 410 |
| cordis-types | 205 / 239 |
| cordis-testkit | 502 / 571 |
| cordis-contract | 31 / 47 |
| console-log | 465 / 515 |
| cordis-watch | 578 / 622 |
| cordis-http | 823 / 892 |
| cordis-window | 943 / 1020 |
| web-api / forward-api / window-api | 25/32；17/21；11/17 |
| kv-api / kv-contract / kv-file | 23/37；56/66；129/142 |
| kv-mem / kv-mem-component | 6/9；67/80 |
| counter / counter-component | 6/9；75/85 |
| Python SDK | 838 / 987 |
| TypeScript SDK / 网页宿主 | 1163/1261；206/231 |
| 核心九个 crate 合计 | 18910 / 21151 |

### M22 外部接入

- 职责：HTTP 方法、头部、状态码、页面属性与网络背压放在 http 插件，依据第一性原理第 3、4 条；大文件接口及文件适配在 stream-api 与语言 SDK，复用已有服务对象转交，依据第 5 条。可信调用方与页面身份由 supervisor/web-deployment 建立，密钥文件及 grants 判定留在 secrets 插件，依据第 3、4、6 条。服务环境变量归 runtime 原有 service install，按名称拒绝机密变量。
- 文档：50cdcd5a 的 README 架构三层表格、官方插件说明和 symbols.caller 对应关系，以及 ADR-0046 §3.11 第 6 条均为设计方新增内容，随 M22 保留并提交。
- 页面外壳：60aa1001 追加 http.title、中性默认页和异常连接提示；guide 与 web skill 补自定义外壳模板。设计方已确认按 document.title、body.innerText、错误正文与响应头检查品牌，协议路径、DOM 挂载标识、导入名与会话对象保持。
- 提取步骤：先把协议描述符的名称、接口和字节计量、远端对象信息查询及诊断额度逻辑从 supervisor 移入 routing。选择该单元是因为它处理 UP/1 的对象描述与保留预算，已有 routing 的 Interface、MetadataBudget 正是其拥有者；内核定义构造、纪元持有与诊断上报继续由 supervisor 负责。公开 Manager 调用路径与签名保持，提取验证随后执行。
- 提取验证：`cargo test --target-dir target/shared -p cordis-supervisor` 的 14 条 manager 与 4 条 process 全部通过，耗时 18.06 / 0.55 秒；日志 `target/http-m22/evidence/supervisor-extraction.log`。格式化后 supervisor 为 5385 / 5828、routing 为 736 / 828，记录 `target/http-m22/evidence/extraction-lines.md`；受信调用来源在此基础上实现。
- 宿主缺陷：真实 HTTP POST 返回 500。host/wire.rs 的 Bridge::invoke 原先把 RPC Future 交给插件自有 Tokio 轮询，rpc.rs:359 的 timeout_at 缺少宿主 reactor；失败记录为 target/http-m22/evidence/smoke-before-host-fix.log。设计方已批准复用 NativeExecutor.start 执行 RPC 并传播取消，依据第一性原理第 3、5 条。普通线程、自有 Tokio 与取消回归纳入本阶段验收；其余 HostV1 入口按同类故障要求核对。
- 同类入口：sleep、plugin、effect_wait、emit_async 的 Future 同样进入宿主执行器；查询/等待可取消，装入、释放与事件登记完成已受理的操作。get、provide、context_derive、effect/defer、on、同步 emit、log、is_current、export_state 已经经 request_sync/NativeExecutor.wait 进入宿主；effect_release 与对象释放使用同步簿记、有界通知或明确持有的 RuntimeHandle。now/config/inherited_state 读取本地状态。Bridge import/export 与 incoming 在已有宿主任务内处理。
- 初次实际行为：三语言路由与重复 Cookie、方法/路径/查询串/头部/正文、405 Allow、路径段匹配、500/502 已通过，`target/http-m22/evidence/routes-test.log`，18.85 秒。三语言各 64 MiB 上传下载逐字节相同、已知长度与分块超限 413、HTTP RSS 增量低于 32 MiB 已通过，`target/http-m22/evidence/files-test.log`，10.82 秒；原结果 `target/test-evidence/files-24929-0/http-sdk/files-24929-0/result.json`。
- 字节表示：依据第一性原理第 5 条和 ADR-0039 的 M22 修订，统一所有 WIT list<u8> 写成字节串，同时接受已有整数数组。SDK Bytes、宏的读取分支、IDL Rust/脚本类型与字段编码已接入，ABI 槽位与布局保持。三语言 record/variant/map/option/result 各 1 MiB、M21 KV 检查点的三语言读取已通过，记录 target/test-evidence/bytes-33517-0/http-sdk/bytes-33517-0/result.json；HTTP 默认 1 MiB 内联体逐字节往返通过，记录 target/test-evidence/routes-48321-0/http-sdk/routes-48321-0/result.json。
- 页面与冲突：target/test-evidence/web-47547-0/http-sdk/web-47547-0/result.json 记录两页 Cookie、page_of、后端授权、伪造保留键、关闭后属性撤回、反向代理、公开来源、两种登记顺序的冲突、最长段前缀、HTTP 强杀重连。default-shell.png、custom-shell-replaced.png、custom-shell-mobile.png、not-found.png 已实际查看；自定义壳替换保持同一文档，390×844 移动视口布局正常。浏览器控制台的 502 来自 HTTP 强杀阶段，404 来自资源缺席场景。
- 定向验收：自有线程 target/test-evidence/threads-34625-0、密钥与服务环境 target/test-evidence/secrets-36047-0、脚手架 target/test-evidence/scaffold-42965-0、通用流 target/test-evidence/streams-45697-0、替换与故障 target/test-evidence/lifecycle-46516-0 均已通过。最后一项在 50 请求/秒期间完成三次替换与失败回滚，验证超时、处理方强杀、其他路由继续服务、恢复登记与最终卸下。

- Y-1/Y-2：`cargo fmt --all --check` 与 `cargo clippy --workspace --all-targets -- -D warnings` 通过；后续 HTTP 职责拆分和夹具调整对应的 `cordis-http`、`cordis-runtime`、`cordis-supervisor` 检查通过，记录 `target/http-m22/evidence/fmt-final.log`、`clippy-complete.log`、`clippy-affected.log`。
- Y-3：工作区覆盖 336 项用例，含 2 项文档用例。初次 `cargo test --workspace` 在 HTTP 413 用例停止；已完成目标的成功结果保留，随后执行 cordis-dev 余下 8 个测试目标，以及排除已完成包的 workspace 余下目标。初次合计 10 项失败均已修复并定向通过，余下用例通过；RM-11 为独立执行的 1 项忽略测量用例。记录 `workspace.log`、`workspace-dev-remaining.log`、`workspace-remaining.log`、`early-doc-tests.log`。汇总排除 console-log 在剩余命令中的 6 项重复结果。
- 413 验收：已知 Content-Length 超限时服务提前返回 413，原客户端继续写正文而遇到 BrokenPipe。用例改为发送超限请求头后读取响应，验证读取正文前的拒绝；分块超限继续实际上传验证。`files-early-response.log` 通过。三语言各 64 MiB 上下行完整字节一致；RSS 从 12,042,240 增至 21,725,184，增量 9,682,944 字节。
- 规范兼容：WIT 夹具按 ADR-0039 使用 Bytes 和字节串期望，Node Buffer 按 Uint8Array 的字节值比较；KV 主版本迁移夹具在方法 Vec 与 Snapshot Bytes 之间转换，持久化断言直接读取 CBOR 字节串。Scoped 夹具按 ADR-0046 接收基础视图的可信身份，复用普通服务方法，并继续验证绑定缓存。通过记录为 `idl-bytes-complete.log`、`native-identity.log`、`runtime-fixtures-fixed.log`、`supervisor-scoped-complete.log`。
- HTTP 结构：启动函数把 Scoped 登记交给 routes 模块，HTTP 服务器按协议入口、静态入口、请求转换及处理方调用拆分。受影响的页面和替换场景在 `http-routes-extraction.log` 通过；最终行为记录为 [页面](measurements/m22-web.json)、[生命周期](measurements/m22-lifecycle.json)。
- Y-4/Y-5：177 条上游用例矩阵与 unsafe 审计通过，记录 `conformance.log`、`unsafe.log`。Y-6/Y-7：`cargo +nightly miri test --target-dir target/shared -p cordis-abi` 的 13 条边界用例与布局比较通过，记录 `miri.log`；原生布局检查在 workspace 中通过。ABI v1.0 源码与布局保持。
- AU-8：相对 df05064 的内核、up、channels、types、process、ABI 差异为空，`frozen-boundary.diff` 为 0 字节；runtime 产品改动限 service install，web-deployment 限页面来源。核心产品依赖树保持插件侧依赖隔离，记录 `core-dependencies.txt`。supervisor/routing 注释行合计为 227→236，记录 `comment-counts.json`。
- RM-11：每语言固定 10000 次 HTTP/1.1 keep-alive GET，范围为客户端发送前至完整响应读取后。Rust/Python/TypeScript 中位数为 472.3125/401.0420/348.3750 µs，P99 为 611.750/478.291/451.667 µs；独立测量期间完成，完整样本见 [RM-11](measurements/m22-latency.json)。
- Y-8：cloc 2.10、`--skip-uniqueness`，按 Rust/Python/TypeScript 规则排除注释和空行；下表记录代码行与总行。supervisor、runtime、routing、host、up、types、SDK 宏保留各自现有职责，超出目标且处于上限以内；cordis-dev 复用既有生成与打包步骤后为 2577 行，其 3000 行上限与职责依据见 ADR-0048。核心九个 crate 合计 19137 / 21406。

| crate | 代码行 / 总行 |
|---|---:|
| console-log | 465 / 515 |
| cordis | 4230 / 5112 |
| cordis-abi | 2407 / 2846 |
| cordis-channels | 250 / 277 |
| cordis-contract | 31 / 47 |
| cordis-dev | 2577 / 2815 |
| cordis-host | 2730 / 3020 |
| cordis-idl | 2331 / 2475 |
| cordis-process | 374 / 410 |
| cordis-routing | 736 / 828 |
| cordis-runtime | 5400 / 5933 |
| cordis-sdk | 888 / 1024 |
| cordis-sdk-macros | 1427 / 1558 |
| cordis-supervisor | 5444 / 5895 |
| cordis-testkit | 502 / 571 |
| cordis-types | 205 / 239 |
| cordis-up | 1628 / 1773 |
| cordis-web-deployment | 870 / 939 |
| counter | 6 / 9 |
| counter-component | 75 / 85 |
| forward-api | 17 / 21 |
| http | 1686 / 1831 |
| http-api | 135 / 152 |
| http-echo | 162 / 176 |
| kv-api | 23 / 37 |
| kv-contract | 56 / 66 |
| kv-file | 135 / 148 |
| kv-mem | 6 / 9 |
| kv-mem-component | 73 / 86 |
| sdks/python/cordis_sdk | 967 / 1162 |
| sdks/typescript/cordis-sdk | 1293 / 1408 |
| sdks/typescript/cordis-web | 207 / 232 |
| secrets | 87 / 97 |
| secrets-api | 6 / 9 |
| stream-api | 110 / 128 |
| watch | 578 / 622 |
| web-api | 25 / 32 |
| window | 943 / 1020 |
| window-api | 11 / 17 |
| 核心进程 crate 合计 | 19137 / 21406 |

- Y-9：README、guide、DOC-13/15、ADR-0039/0041/0046/0048、skill、解释页与页面流程同步。skill 的 quick_validate 通过；三语言 HTTP 模板已真实构建运行。设计方 ADR-0047 与阶段表一并提交，M23 按后续顺序执行。
- 可视资料：新增四张业务画面已查看并写入 `docs/design/flow/screenshots/`。流程图的应用外壳组、详情导航及移动布局已查看，文档画面在 `target/http-m22/evidence/docs/`；内置浏览器服务返回不可用后沿项目既有 Playwright 流程执行，浏览器进程在 finally 中退出。
- 发布与缓存：v0.1.0 等待 M22 复核后发布，原 M21 release 编译作为早期准备保留；发布构建将使用 M22 源码及路径重映射。当前编译缓存服务于复核和发布，清理按已授权的发布后顺序执行。

## 第二阶段候选与恢复记录

用户设置的 10 分钟计时于 2026-11-12 02:40:22 UTC 到期后暂停，随后按用户指令恢复。设计提交为 `478b3a4`，包含用户指定的序号和完整字节比对方案。M7 已完成验收，后续候选继续按下表接入。

| 工作项 | 保存位置与实际状态 | 恢复入口 |
|---|---|---|
| M8 协议与实现 | `crates/cordis-abi/`、DOC-12、ADR-0015、`docs/abi/v1-layout.txt`、安全审计工具 | 已通过本节记录的行为、Miri、布局和 Rust 1.85 验证 |
| M9 实现 | 正式 SDK、宏及三个示例已完成；候选副本在 `target/sdk-m9/` | 本节的构建、工作区测试、Miri 与 Rust 1.85 验证通过 |
| 宿主复用资料 | `target/loader-m10/`；内核增补在候选内核与 kernel.patch | 8 条真实动态库用例与 5 条内核依赖用例有成功证据。按 Q0 把控制逻辑移入核心监管，本地库、租约和 ABI 执行用于新 M11；宿主产品依赖与内核分离 |
| 运行时复用资料 | `target/runtime-m11/` | 进程内候选 check 与真实子进程综合测试通过；配置、目录监视、示例资料供新 M12 复用，控制入口改用核心监管 |
| M10 协议 | `target/up-m10/` 与协议草案 | 8 条 codec 用例通过。up1_decoder 以 9 个产品编码种子执行固定时限 fuzz，09:15:28–09:45:29 UTC，632926 次输入、1801 秒、退出 0；cov 2195、ft 5906、corpus 1340 项约 21MiB，RSS 742MiB。PID 94944/95360 已确认退出，结果与语料保留在该候选的 fuzz 目录 |
| M10 连接与监管 | 正式 crates/cordis-up、crates/cordis-supervisor；旧候选保留在 target | 已完成本节验收。后续修改以正式源码为准；旧 candidate RPC 保留编码超限关闭连接的修复前证据 |
| M11 原生宿主 | `target/host-m11/` | 26 槽基础已完成，同组 typed=true、跨组 typed=false 均实际完成 100→102 并取得快照102；使用方采用 Rust1.85.0 构建。日志在 build/native-basic.log，后续继续新增组合能力与完整 Host 验收 |

M8 候选保留独立 `[workspace]` 以便隔离编译，正式接入时改为根工作区成员及依赖继承。候选解析版本为 serde 1.0.229、ciborium 0.2.2、syn 2.0.119、proc-macro2 1.0.107、quote 1.0.47；审计解析器归开发依赖。SDK、ABI、宏及示例插件的 MSRV 保持 1.85，实际跨编译器验收在 AC-8。

后续设计按 ADR-0016–0021 与已确认 DOC-15 执行。依赖版本候选为 clap 4.6.7、semver 1.0.28、toml 1.2.1、libloading 0.9.0、notify 8.2.0、nix 0.31.3；模糊测试使用 libfuzzer-sys 0.4.13。

上述候选的源码、日志、语料及验收资料按引用保留，可重建程序与缓存已按 M21 清理记录释放。

## 公开接口清单

| 对象 | 公开能力 |
|---|---|
| Context | 根创建、纤程句柄、类型化扩展、隔离域、拦截元数据、效应、服务、组件、事件、日志、宿主时间和错误缓冲 |
| Fiber / FiberState / Epoch | 名称、存活与稳定状态、配置及规格、效应树、目标纪元、等待、更新、重启和释放 |
| Effect / EffectScope / EffectMeta | 同步及异步逆变换、子效应归属、当前纪元检查、等待执行、显式释放和元信息 |
| ServiceKey / Merge / Realm | 服务值与元数据类型、元数据合并、默认域及独立域 |
| Scoped / Provision | 值访问、caller/requester/definition、依赖与元数据解析、日志来源、提供方修改、通知与撤回 |
| Component / ComponentDef / Config / Inject | 静态和闭包组件、原始配置解析、声明依赖和元数据、组件身份 |
| Registry | 实例登记查询、组件定义查询、纤程列表、原地替换及别名转发 |
| Event / 三种 Kind / EventOptions / Next | 类型化参数与结果、五种派发、过滤、一次性监听、瀑布延续，以及 11 种内核事件 |
| Logger / LogMessage / LogValue / WeakFiber | 四级日志、命名、环形缓冲、导出器、过滤、错误展开与弱来源 |
| Error / ValidationError / ValidationIssue / Result | 可克隆错误、登记位置、错误原因、聚合成员及配置问题路径 |
| Spawn / Clock / Task / TestExecutor | 宿主任务提交、时间与异步等待、可发送任务、确定性调度和逻辑时间推进 |

## 决策与发现

实现过程中得到的、文档里没有的结论记在这里，每条写明日期、结论、依据（源码位置或用例编号）。需要改变 `DOC-01` 条款或 `DOC-02` 规则的结论另写 ADR，这里只留一行指向它。

- 2026-11-13（Asia/Shanghai）：完整独立 Python/Node 发布包按 ADR-0043 设置解释器权限和签名顺序，真实包内扩展、JIT、KV 服务与退出通过。原生故障测试改为状态通知及连接结束驱动，全量负载期间五轮各 14 条通过，见 M21 稳定性记录。
- 2026-11-13（Asia/Shanghai）：RM-9 首轮与评审全量负载重叠，停载重测保持原正式实现与原判据并通过；两轮原始样本并列保存。前期轮次偏高和比值约 12% 变动记为方法局限，ADR-0045 静态分派与测量方法研究保持后续独立事项。
- 2026-11-13（Asia/Shanghai）：M21 提交前按 e5e4f5cd、6990ee77 完成 612 项缓存与旧二进制清理，`du -sh target` 60G→30G，目录占用减少 31,761,160 KiB，文件系统可用空间实测增加 31,625,617,408 字节；记录为 `docs/measurements/m21-cleanup.json`。window-recovery 目录已按“用户 2026-11-13 授权删除”释放，AU-17 归档结果和截图保留。
- 2026-11-13（Asia/Shanghai）：M20 页面使用 Context 扩展的命名空间隔离显式命名域，前后端事务共同保存页面状态；生命周期稳定检查按实际 Instance 身份处理退役页面，失败 Owner 通过恢复序号重新激活。ADR-0041 与八个浏览器场景记录对应结构与行为。
- 2026-11-13（Asia/Shanghai）：配置移除提供方产生的 Pending 是有效部署状态，版本替换继续沿 SW-7 检查 Active；两者共享提交与恢复流程，判据见 ADR-0044。设计方指出的 PIP_REQUIRE_VIRTUALENV 环境依赖由公共 testkit 安装函数处理，三处真实场景已通过。

- 2026-11-12（UTC−08:00）：M19 恢复后完成旧候选清理，10 个带 Cargo 标记的 debug/release 目录已删除，`du -sh target` 从 41G 降至 38G；路径与结果为 `target/cache-cleanup-m19.json`。编译目录内的结果文件保存到 `target/cache-cleanup-m19-evidence`。默认 target/debug、target/shared、Miri、窗口辅助程序及 M20 正在使用的 agent-shared/agent-cleanup 保持；后两者分别承担路由提取和网页接入。
- 2026-11-13（Asia/Shanghai）：按 e7df894d 清理已结束测试现场，共移除 264 个目录，其中 262 个属于文档未引用现场，2 项引用误归类的处理见后条。13 个引用现场保留文本、JSON、CBOR 与图像证据，清除 1234 个编译产物及依赖文件。`du -sh target` 从 39G 降至 32G，执行记录为 `target/test-residue-cleanup-m19.json`，脚本为 `target/prune_test_residue_m19.py`。公共守卫与测试调用迁移进行中，随后按要求连续执行两次工作区测试。
- 2026-11-13（Asia/Shanghai）：旧现场清理的引用匹配漏识别全角括号，behavior-55112、scaffold-55112 两个 M19 进行中目录被误归入移除项；全局运行日志仍在。匹配已改为路径字符边界，当前状态引用第一轮完整执行后由守卫归档的新证据。旧目录的原始文件保持删除事实，新的归档来自实际重跑结果。
- 2026-11-13（Asia/Shanghai）：补充清理同类旧现场，38 个未引用目录移除，14 个引用目录保留证据，删除 405 个编译产物；`du -sh target` 从 33G 降至 32G，记录为 `target/test-residue-additional-m19.json`。已交付的 `target/window-tests/recovery-51816/dist/window-recovery.app` 保持完整。

- 2026-11-12（UTC−08:00）：设计方按用户授权清理旧候选与复核的 Cargo 缓存，`du -sh target` 从 68G 降至 27G；执行脚本与演练记录为 `target/claude-edits/clean_caches.py`、`clean-dryrun.txt`。后续构建按 AGENTS 规则 11 共用各编译根的 shared 目录，阶段结束保留编译缓存。
- 2026-11-12（UTC−08:00）：M17 复核后的编译根收拢已执行。删除前进程查询确认 main-build 处于空闲，主会话统一使用默认 `target/` 与 `target/shared`，`target/main-build`、`target/m17-review/source` 已删除。原隔离现场的日志、JSON、CBOR 和生成源码迁至 `target/m17-review/evidence/isolated-target`，646 个可重建二进制及发布归档移除清单为该 evidence 目录的 artifact-cleanup.json。`du -sh target` 从 41G 降至 29G，保留证据 38M；默认 target/debug 18G，agent-shared 418M，agent-cleanup 的编译目录处于缺席状态。M17 段落中的原始源码与证据路径对应本次迁移前位置。

- 2026-11-11（UTC−08:00）：第二阶段按用户决定采用单调序号路径与完整字节比较；验证门保存已验证库副本，印记记录副本路径、套件及断言结果。DOC-11、DOC-13、DOC-14、ADR-0012 已同步。
- 2026-11-11（UTC−08:00）：按 DOC-10 §8 统一导出冻结语义：SW-1–SW-4 保持实例与绑定，SW-5 可逆冻结，导出失败解冻。kv-mem/kv-file 使用共同逻辑插件名 kv、组件名 kv-store，使 AC-2 满足同名替换输入。K-5 已在 940650c 实现。
- 2026-11-11（UTC−08:00）：M8 冻结前补齐 ABI 的释放错误通道、缓冲租约、异步结果和回调签名，形成 ADR-0015 后实现；M9 接口宏通过 trait/impl 的 async 方法转换提供 dyn 接口，接口 crate 保持普通 Rust 库。
- 2026-11-12（Asia/Shanghai）：Miri 确认完整虚表指针需要覆盖方法槽，描述符释放前结束受保护借用，组件表指针从最终分配位置发布；相应修复后的 12 条边界用例通过。布局测试采用编译期基准，适配 Miri 文件隔离。
- 2026-11-12（Asia/Shanghai）：项目所有者确认 DOC-15 与 ADR-0018–0020，Q0 将全部插件代码移入核心之外的进程。M9 解除 b792b2e 的 ABI 冻结，加入 V-1–V-4，候选布局在新 M11 的跨库验收后冻结；后续里程碑按 DOC-15 §15 推进到 M14。
- 2026-11-12（Asia/Shanghai）：通用调用的服务值使用 CBOR 标签 60000 包裹宿主分配的对象编号。Host 的 export/import/release 管理编号引用；输入载荷和结果缓冲持有导出引用，接收方导入独立对象。AbiBuf 增加源侧 owner 清理上下文，使取消和解码失败沿 RAII 回收编号与库租约；该冻结前布局细化随 M9 验证。
- 2026-11-12（Asia/Shanghai）：核心进程保存系统唯一内核。cordis-host 使用普通结构与 RAII 管理库和 ABI 对象，所有实例决策、服务依赖、get/provide 与效应登记由核心经 UP/1 管理；M11 检验同组替换的消费者优先清理顺序和直连引用更新。
- 2026-11-12（Asia/Shanghai）：UP/1 连接使用通用异步读写接口，资源输入由 Payload 持有；Outgoing 在取得发送队列位置后转交引用。ADR-0023 的前端转发通道在 M10 定义规范，M20 实现；ADR-0024 的开发工具、管理授权与 watch 插件按 M12 接入。
- 2026-11-12（Asia/Shanghai）：依赖快照新增的 Arc 在内核锁外释放；两条析构观察用例在修复前观察到锁被持有，引用遍历修复后通过。上下文使用 Generation 引用寿命覆盖全部清理，适配顶层效应并发等待的 S-FBR-12。
- 2026-11-12（Asia/Shanghai）：冻结前补充 effect_release 宿主槽与 UP/1 通知，SDK 最后一份 Effect 引用归还外部句柄，实际效应由内核继续持有至清理。此项使长运行实例的局部效应循环按实际资源寿命回收记录，随 M10 验证。
- 2026-11-12（Asia/Shanghai）：效应句柄回归固定执行三次局部清理，旧 SDK 留下 3 个外部记录，Arc 最后引用释放修复后记录归零。失败与成功输出保存在 target/effect-handle-before.log、target/effect-handle-after.log；原 SDK 回环复测通过，覆盖 26 个 Host 槽。
- 2026-11-12（Asia/Shanghai）：DOC-15 原生清单样例的 name 按 DOC-11 的共同逻辑插件名改为 kv，与 kv-file 描述符及清单一致性条款相符。M12 配置与清单解析候选位于 target/runtime-m12，首次 check 通过，7.86 秒；相关行为与模糊验证随 M12 接入。

- 2026-11-11：`DSP-03` 包含 `root.on`，其嵌套效应归属、元信息树与逆序释放属于 M1；事件表登记与移除在 M2 接入。依据 `dispose.spec.ts:37-74`。
- 2026-11-11（UTC−08:00）：`CMP-10` 在挂载后立即释放根，仍执行一次子组件加载；卸载调用逆变换前的调度让步决定该顺序。已按 `fiber.ts:495-499` 补正 S-FBR-12 并恢复原始测试触发。
- 2026-11-11（UTC−08:00）：内核派发的 InternalService 监听器 panic 按 A-9 记录，绑定建立与撤回继续完成；调用方直接 emit 的 panic 传播语义沿用 M2。
- 2026-11-11（UTC−08:00）：压力测试确认提供方状态通知完成前存在等待提前返回窗口。稳定判定现覆盖整段 InternalStatus 与服务通知，通知后重新判断目标；确定性屏障用例及原定 8 线程 1000 轮验证均通过。
- 2026-11-11（UTC−08:00）：`DrainGuard::drop` 统一交还调和权；正常路径重新争取退出窗口内的新请求，panic 后残留队列由下一次 refresh/drain 接续。内部持锁 panic 用例修复前在复位断言处失败，修复后与全部 183 条运行测试、2 条文档测试通过；8 线程 1000 轮耗时 2.87 秒，X-1–X-6 全部通过。当前内核 4574 行，其中 fiber.rs 1035 行、kernel.rs 151 行，其余模块行数沿用 M6 表。
