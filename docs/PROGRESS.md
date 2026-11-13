# 进度日志

本文件记录实现的当前位置，供中断后继续。每个里程碑结束时更新；里程碑进行中发生需要后来者知道的事情时也更新。更新时改写“当前状态”一节，并在“里程碑记录”中补充对应小节，不在文件末尾堆叠流水账。

## 当前状态

| 项 | 值 |
|---|---|
| 最近完成的实现与检查 | `M16`；发布包、HTTP、窗口与运行目录已接入正式工作区，阶段检查通过 |
| 当前里程碑 | M17 接口描述语言；Codex skill 链接已按授权安装 |
| 下一步 | M17 接口描述语言：接入 WIT 与宏职责拆分候选，按正式差异执行对应验收 |
| 遗留验收 | AU-17 的截图与 Cmd-Q 命令分别返回屏幕录制、辅助功能权限请求；用户已许可记录缺项并继续交付 |
| 最后更新 | 2026-11-12（America/Los_Angeles），M16 阶段检查、授权界面命令结果与测量记录 |

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
- Y-10：提交主题 `M16: 实现发布打包与独立窗口进程`，推送目标 origin/main；AU-17 的系统权限缺项按用户许可保留。
- AU-17 授权界面验收：按用户授权运行 target/window-m16/inspect_window.swift，对本次窗口 PID 读取系统应用信息、截图和发送 Cmd-Q。窗口 PID 64939→65170，982 ms 恢复，核心 PID 64904 保持，HTTP/KV context 保持 5/7。[恢复前应用信息](measurements/m16-window-app-before.json)和[恢复后应用信息](measurements/m16-window-app-after.json)的系统列表各记录一个 Regular 进程。截图命令返回 1，原因是屏幕录制权限等待允许；Cmd-Q 命令返回 1，原因是辅助功能权限等待允许，按键发送停在权限请求。真实画面、Dock 外观及快捷键退出结果保留缺项。随后以 SIGTERM 有序退出，返回 0，5 个进程均退出，应用签名有效。[授权执行结果](measurements/m16-window-authorized.json)保留本次实际观测，原始信息、错误输出和结果位于 target/window-m16/authorized-{before,after,result}.json、authorized-capture.stderr.log 与 authorized-quit.stderr.log。
- 阶段状态：M16 实现与阶段检查完成；用户已明确许可 GUI 命令失败时记录缺项并继续交付。M15 的 Codex 用户 skill 链接已按授权安装，路径与创建结果见上一节。M16 的提交与推送在本阶段交付时记录。
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

当前保留 `target/` 中的候选源码及缓存，继续后续里程碑。

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
