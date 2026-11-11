# 04 一致性矩阵

| 项 | 内容 |
|---|---|
| 文档编号 | `DOC-04` |
| 状态 | 进行中。§4 的表格由实现者在每个里程碑更新 |
| 读者 | 实现者、评审者 |
| 输入 | `UPSTREAM` 的 16 个 spec 文件，位于 `cordis/packages/core/tests/`，commit `261c5fd` |
| 输出 | 177 条上游用例各自的处置状态与对应的 Rust 测试 |
| 校验 | `conda run -n base python tools/conformance.py check [--through M<n>]` |

## 1 用途

矩阵是目标 `G2`（行为与 `UPSTREAM` 一致）的验收物。上游的每一条 `it(...)` 声明在矩阵中占一行。`it.each(...)` 声明占一行，Rust 侧为其每个参数各写一个测试或写成一个参数化测试。

## 2 字段

| 列 | 含义 | 填写者 |
|---|---|---|
| ID | 稳定编号，前缀标识 spec 文件，序号为文件内出现顺序 | 工具生成，不得修改 |
| 上游位置 | `文件:行号` | 工具生成，不得修改 |
| 上游用例 | `it` 的标题原文 | 工具生成，不得修改 |
| 里程碑 | 该用例应在哪个里程碑离开“未开始” | 工具生成；需要调整时同时修改 `tools/conformance.py` 的 `OVERRIDES` |
| 状态 | 四个取值之一，见 §3 | 实现者 |
| Rust 测试 / 理由 | 测试位置写作 `crates/cordis/tests/<文件>.rs::<函数名>`；已改写与不适用的行在此写明理由 | 实现者 |

## 3 状态与判定步骤

| 状态 | 含义 | 要求 |
|---|---|---|
| 未开始 | 尚未处理 | 无 |
| 已移植 | Rust 测试的场景与全部断言同上游一致，只有语法不同 | 填写测试位置 |
| 已改写 | Rust 测试检验同一条语义，但场景或断言因宿主语言差异而调整 | 填写测试位置与理由。理由指出调整了什么，以及它检验 `DOC-01` 的哪一条 |
| 不适用 | 该用例检验的行为在 Rust 中没有对应物 | 填写理由。理由指出该行为依赖 JS 的哪一项特性，以及 `DOC-02` §7 映射表的哪一行说明了替代方式 |

处理每条用例时按顺序判定：

1. 读上游用例的源码，写出它检验的语义，对应到 `DOC-01` 的条款。找不到对应条款时，先在 `DOC-01` 补一条 `S-*` 条款并注明源码位置。
2. 该语义在 Rust 接口中可以用相同的场景表达：状态取“已移植”。
3. 该语义存在，但场景依赖 JS 特性（属性访问代理、`this` 绑定、原型链、装饰器、`util.inspect`、未处理拒绝）：按 `DOC-02` §7 的映射改写场景，状态取“已改写”。
4. 用例检验的就是 JS 特性本身（例如原型属性名作为事件名不发生污染）：状态取“不适用”。

“不适用”的数量预期集中在 `shadow`、`associate`、`invoke`、`decorator` 四个文件和 `events` 中的个别用例。其余文件出现“不适用”时，在 `docs/PROGRESS.md` 单独说明。

## 4 矩阵

下列表格由 `conda run -n base python tools/conformance.py init` 生成，初始状态全部为“未开始”。

M0 使用原有清单核对 16 个 spec 文件的 177 条用例；用例实现从 M1 开始。

### DSP：`dispose.spec.ts`（15 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| DSP-01 | `dispose.spec.ts:7` | dispose by component | M3 | 已改写 | `crates/cordis/tests/component.rs::component_disposes_its_effect_once`；显式效应作用域表达释放函数，检验 S-EFF-4。 |
| DSP-02 | `dispose.spec.ts:23` | dispose manually | M1 | 已移植 | `crates/cordis/tests/dispose.rs::dispose_manually` |
| DSP-03 | `dispose.spec.ts:37` | yield dispose | M1 | 已改写 | `crates/cordis/tests/dispose.rs::yield_dispose`；生成器交回释放函数改为 defer/adopt，检验 S-EFF-3、S-EFF-6、S-EFF-7 的归属、元信息与逆序组合；使用类型化事件登记监听器。 |
| DSP-04 | `dispose.spec.ts:76` | async return 1 | M3 | 已改写 | `crates/cordis/tests/dispose_async.rs::async_return`；Future 与 defer 表达异步返回释放函数，检验 S-EFF-1、S-EFF-4。 |
| DSP-05 | `dispose.spec.ts:90` | async return 2 | M3 | 已改写 | `crates/cordis/tests/dispose_async.rs::async_return`；在途 Future 持续至完成，释放等待逆变换登记，检验 T-EFF-5。 |
| DSP-06 | `dispose.spec.ts:103` | async yield 1 | M3 | 已改写 | `crates/cordis/tests/dispose_async.rs::async_yield_and_wait_for_execution`；显式 defer 步骤替代异步生成器，检验 T-EFF-3。 |
| DSP-07 | `dispose.spec.ts:123` | async yield 2 (aborted) | M3 | 已改写 | `crates/cordis/tests/dispose_async.rs::async_yield_cooperative_stop`；is_current 在步骤边界协作退出，检验 T-EFF-4。 |
| DSP-08 | `dispose.spec.ts:143` | async yield 3 (aborted) | M3 | 已改写 | `crates/cordis/tests/dispose_async.rs::async_yield_cooperative_stop`；保留停止前在途步骤交回的逆变换，检验 S-EFF-5。 |
| DSP-09 | `dispose.spec.ts:165` | async yield (aborted, finally) | M3 | 已改写 | `crates/cordis/tests/dispose_async.rs::async_yield_finalization`；显式收束逻辑替代生成器 finally，检验 S-EFF-5 的恢复顺序。 |
| DSP-10 | `dispose.spec.ts:185` | async yield 4 (await dispose) | M3 | 已改写 | `crates/cordis/tests/dispose_async.rs::async_yield_and_wait_for_execution`；Effect::settled 替代 Promise 同化，检验 S-EFF-8。 |
| DSP-11 | `dispose.spec.ts:205` | return with error | M1 | 已改写 | `crates/cordis/tests/dispose.rs::return_with_error`；JS throw 改为 Result，检验 S-EFF-2 的错误传播和登记位置。 |
| DSP-12 | `dispose.spec.ts:217` | yield with error | M1 | 已改写 | `crates/cordis/tests/dispose.rs::yield_with_error`；生成器 yield 后 throw 改为 defer 后返回 Err，检验 S-EFF-2 的部分恢复。 |
| DSP-13 | `dispose.spec.ts:230` | async return with error | M3 | 已改写 | `crates/cordis/tests/dispose_async.rs::async_error_recovers_registered_steps`；Result 表达异步错误，检验 S-EFF-2。 |
| DSP-14 | `dispose.spec.ts:242` | async yield with error | M3 | 已改写 | `crates/cordis/tests/dispose_async.rs::async_error_recovers_registered_steps`；defer 与 Result 表达生成器部分失败，检验 S-EFF-2。 |
| DSP-15 | `dispose.spec.ts:261` | async yield invalid effect | M3 | 已改写 | `crates/cordis/src/effect.rs::EffectScope::defer`；compile_fail 文档测试检查非法效应值；闭包类型约束替代运行期 Invalid effect，DOC-02 §5.2、S-EFF-1。 |

### EVT：`events.spec.ts`（31 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| EVT-01 | `events.spec.ts:18` | deprecated dispatch returns bound callbacks that respect later removal | M2 | 已改写 | `crates/cordis/tests/events.rs::all_modes_skip_removed_listeners`；旧 dispatch 回调数组改为派发快照，检验 S-EVT-2。 |
| EVT-02 | `events.spec.ts:35` | deprecated waterfall dispatch continues past a removed callback | M2 | 已改写 | `crates/cordis/tests/events.rs::waterfall_skips_removed_listener`；旧 dispatch 回调数组改为瀑布快照，检验 S-EVT-8。 |
| EVT-03 | `events.spec.ts:49` | supports symbol event names across dispatch modes | M2 | 已改写 | `crates/cordis/tests/events.rs::typed_keys_across_dispatch_modes`；Symbol 改为类型键，跨派发族分别使用同步与异步键，检验 S-EVT-1、S-EVT-4–S-EVT-7。 |
| EVT-04 | `events.spec.ts:66` | supports symbol event names with ctx.once() | M2 | 已改写 | `crates/cordis/tests/events.rs::once_and_explicit_disposal`；Symbol 改为类型键，检验 S-EVT-9。 |
| EVT-05 | `events.spec.ts:77` | treats prototype property names as ordinary events | M2 | 不适用 | 用例检验 JavaScript 对象原型属性污染；DOC-02 §7 规定 Rust 事件由类型 E 标识。 |
| EVT-06 | `events.spec.ts:96` | removes empty event buckets after disposal | M2 | 已改写 | `crates/cordis/tests/events.rs::empty_buckets_are_removed`；Symbol 与 Reflect.has 改为类型键与事件表计数，检验 S-EVT-1。 |
| EVT-07 | `events.spec.ts:108` | ctx.on() | M2 | 已移植 | `crates/cordis/tests/events.rs::on_and_explicit_disposal` |
| EVT-08 | `events.spec.ts:121` | ctx.once() | M2 | 已移植 | `crates/cordis/tests/events.rs::once_and_explicit_disposal` |
| EVT-09 | `events.spec.ts:134` | ctx.once() runs once when an earlier listener emits recursively | M2 | 已移植 | `crates/cordis/tests/events.rs::once_is_consumed_by_recursive_dispatch` |
| EVT-10 | `events.spec.ts:149` | ctx.once() runs once across concurrent serial dispatches | M2 | 已改写 | `crates/cordis/tests/events.rs::once_across_concurrent_serial_dispatches`；Promise 屏障改为确定性 Clock 等待，检验 S-EVT-9。 |
| EVT-11 | `events.spec.ts:163` | %s skips listeners removed during dispatch | M2 | 已改写 | `crates/cordis/tests/events.rs::all_modes_skip_removed_listeners`；四种派发分别使用同步与异步事件族，检验 S-EVT-2。 |
| EVT-12 | `events.spec.ts:177` | %s defers added listeners until the next dispatch | M2 | 已改写 | `crates/cordis/tests/events.rs::all_modes_defer_added_listeners`；四种派发分别使用同步与异步事件族，检验 S-EVT-2。 |
| EVT-13 | `events.spec.ts:188` | skips a listener disposed by an earlier component during dispatch | M3 | 已改写 | `crates/cordis/tests/events_lifecycle.rs::serial_skips_listener_disposed_with_component`；ComponentDef 表达组件，检验 S-EVT-2 的释放后跳过。 |
| EVT-14 | `events.spec.ts:200` | waterfall continues past a removed listener | M2 | 已移植 | `crates/cordis/tests/events.rs::waterfall_skips_removed_listener` |
| EVT-15 | `events.spec.ts:219` | waterfall defers added listeners until the next dispatch | M2 | 已移植 | `crates/cordis/tests/events.rs::waterfall_defers_added_listener` |
| EVT-16 | `events.spec.ts:233` | waterfall continues after a recursive dispatch has consumed a once listener | M2 | 已移植 | `crates/cordis/tests/events.rs::waterfall_recursion_consumes_once` |
| EVT-17 | `events.spec.ts:251` | internal/update continues past a removed fiber listener | M3 | 已改写 | `crates/cordis/tests/events_lifecycle.rs::update_skips_removed_fiber_listener`；RawConfig 与类型化瀑布表达更新，检验 S-EVT-11。 |
| EVT-18 | `events.spec.ts:280` | ctx.parallel() | M2 | 已改写 | `crates/cordis/tests/events.rs::parallel_filters_and_aggregates_errors`；this 过滤改为 Context 过滤、throw 改为 Err、定时器改为 Clock，检验 S-EVT-3、S-EVT-5。 |
| EVT-19 | `events.spec.ts:311` | ctx.parallel() labels symbol events | M2 | 已改写 | `crates/cordis/tests/events.rs::parallel_labels_typed_event`；Symbol 标签改为 Event::NAME，检验 S-EVT-5。 |
| EVT-20 | `events.spec.ts:322` | ctx.emit() | M2 | 已改写 | `crates/cordis/tests/events.rs::synchronous_filters_and_panic_propagation`；this 过滤改为 Context 过滤，调用方接收 panic，检验 S-EVT-3、S-EVT-4、A-9。 |
| EVT-21 | `events.spec.ts:341` | ctx.serial() | M2 | 已改写 | `crates/cordis/tests/events.rs::serial_filtering_errors_and_bail_result`；this 过滤改为 Context 过滤、异常改为 Err，检验 S-EVT-3、S-EVT-7。 |
| EVT-22 | `events.spec.ts:360` | ctx.bail() | M2 | 已改写 | `crates/cordis/tests/events.rs::synchronous_filters_and_panic_propagation`；this 过滤改为 Context 过滤，调用方接收 panic，检验 S-EVT-3、S-EVT-6、A-9。 |
| EVT-23 | `events.spec.ts:379` | ctx.waterfall() | M2 | 已移植 | `crates/cordis/tests/events.rs::waterfall_nesting_and_short_circuit` |
| EVT-24 | `events.spec.ts:407` | ctx.waterfall() rejects duplicate next() | M2 | 已改写 | `crates/cordis/tests/events.rs::waterfall_rejects_duplicate_next`；重复调用异常改为 Result 错误，检验 S-EVT-8。 |
| EVT-25 | `events.spec.ts:421` | ctx.waterfall() rejects continuations from outer frames | M2 | 已改写 | `crates/cordis/tests/events.rs::waterfall_rejects_outer_continuation`；共享 Next 句柄承载延续并返回 Result 错误，检验 S-EVT-8。 |
| EVT-26 | `events.spec.ts:442` | ctx.waterfall() rejects duplicate next() after awaiting | M2 | 已改写 | `crates/cordis/tests/events.rs::waterfall_rejects_duplicate_after_await`；Task 返回值与 Result 错误表达异步延续，检验 S-EVT-8。 |
| EVT-27 | `events.spec.ts:455` | ctx.waterfall() supports nested async calls | M2 | 已改写 | `crates/cordis/tests/events.rs::waterfall_supports_nested_async_calls`；异步瀑布返回 Task<Result>，检验 S-EVT-8。 |
| EVT-28 | `events.spec.ts:473` | does not accumulate across reloads | M3 | 已改写 | `crates/cordis/tests/events_lifecycle.rs::update_listeners_are_replaced_on_reload`；ComponentDef 与 RawConfig 表达重载配置，检验 S-EVT-11。 |
| EVT-29 | `events.spec.ts:490` | can be disposed explicitly | M3 | 已改写 | `crates/cordis/tests/events_lifecycle.rs::update_listener_can_be_disposed`；Effect 句柄表达显式移除，检验 S-EVT-11。 |
| EVT-30 | `events.spec.ts:507` | runs prepended listeners first | M3 | 已改写 | `crates/cordis/tests/events_lifecycle.rs::update_prepended_listeners_run_first`；类型化瀑布表达更新，检验 S-EVT-11 的头插顺序。 |
| EVT-31 | `events.spec.ts:534` | global listeners are not tied to a child fiber reload | M3 | 已改写 | `crates/cordis/tests/events_lifecycle.rs::global_update_listener_survives_child_reload`；ComponentDef 与 RawConfig 表达组件和配置，检验 S-EVT-11 的全局归属。 |

### CMP：`component.spec.ts`（11 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| CMP-01 | `component.spec.ts:8` | apply functional component | M3 | 已改写 | `crates/cordis/tests/component.rs::functional_component_receives_configuration`；ComponentDef 闭包接收类型化配置，检验 S-REG-2。 |
| CMP-02 | `component.spec.ts:18` | apply object component | M3 | 已改写 | `crates/cordis/tests/component.rs::typed_component_receives_configuration`；Component trait 替代对象 apply，检验 S-REG-2。 |
| CMP-03 | `component.spec.ts:29` | apply component through deprecated ctx.plugin() | M3 | 已改写 | `crates/cordis/tests/component.rs::functional_component_receives_configuration`；废弃 plugin 别名统一映射为 DOC-02 §7 的 mount，检验 S-REG-2。 |
| CMP-04 | `component.spec.ts:40` | apply invalid component | M3 | 已改写 | `crates/cordis/src/registry.rs::Context::mount`；compile_fail 文档测试检查非法组件类型；Component trait 替代动态对象检查，DOC-02 §7、S-REG-2。 |
| CMP-05 | `component.spec.ts:47` | inactive context | M3 | 已改写 | `crates/cordis/tests/component.rs::disposed_context_rejects_new_registrations`；Result 表达失活访问错误，检验 S-FBR-3。 |
| CMP-06 | `component.spec.ts:61` | context inspect | M3 | 已改写 | `crates/cordis/tests/component.rs::context_debug_uses_nearest_component_name`；Debug 替代 util.inspect，检验 S-FBR-4。 |
| CMP-07 | `component.spec.ts:88` | ctx.registry | M3 | 已改写 | `crates/cordis/tests/component.rs::registry_tracks_runtime_instances`；运行时登记和移除的可观察行为检验 S-REG-1。 |
| CMP-08 | `component.spec.ts:97` | nested components | M3 | 已改写 | `crates/cordis/tests/component.rs::nested_components_release_their_event_handlers`；ComponentDef 与类型化事件表达嵌套，检验 T-LIF-8。 |
| CMP-09 | `component.spec.ts:134` | compare snapshot | M3 | 已改写 | `crates/cordis/tests/component.rs::component_disposal_restores_registration_snapshot`；比较效应、注册表与事件状态，检验 T-CTX-1。 |
| CMP-10 | `component.spec.ts:155` | root dispose | M3 | 已改写 | `crates/cordis/tests/component.rs::root_disposal_releases_children_and_remains_active`；显式等待根释放任务，检验 S-FBR-17。 |
| CMP-11 | `component.spec.ts:175` | Service.init | M4 | 已改写 | `crates/cordis/tests/service.rs::cmp11_component_initialization_registers_release`；Service.init 方法映射为组件初始化与显式 defer，检验 S-SVC-13、S-EFF-4。 |

### FBR：`fiber.spec.ts`（22 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| FBR-01 | `fiber.spec.ts:7` | inertia lock 1 | M4 | 已改写 | `crates/cordis/tests/service.rs::fbr01_fbr02_inertial_loading_and_unloading`；生成器释放改为 defer_async，保留 400/800/1200/2200/3200 毫秒阶段，检验 T-LIF-6、S-FBR-8。 |
| FBR-02 | `fiber.spec.ts:27` | inertia lock 2 | M4 | 已改写 | `crates/cordis/tests/service.rs::fbr01_fbr02_inertial_loading_and_unloading`；以类型化绑定替换表达同提供方新身份，检验 T-LIF-2、T-LIF-6。 |
| FBR-03 | `fiber.spec.ts:49` | inertia lock 3 | M4 | 已改写 | `crates/cordis/tests/service.rs::fbr03_active_consumer_returns_to_pending`；Service 类改为提供服务的组件，确定性时钟驱动加载和清理，检验 S-FBR-12、T-COE-6。 |
| FBR-04 | `fiber.spec.ts:71` | component error | M3 | 已改写 | `crates/cordis/tests/fiber.rs::component_failure_cleans_partial_effects`；资源序列与 Result 检验 S-FBR-11。 |
| FBR-05 | `fiber.spec.ts:93` | failed fiber does not re-enter on dependency refresh | M4 | 已改写 | `crates/cordis/tests/service.rs::fbr05_fbr06_failed_fiber_recovers_through_update`；真实配置控制加载结果，使用实际计数记录加载，检验 S-FBR-7。 |
| FBR-06 | `fiber.spec.ts:108` | update recovers a failed fiber | M4 | 已改写 | `crates/cordis/tests/service.rs::fbr05_fbr06_failed_fiber_recovers_through_update`；通过新配置恢复失败组件，替代 JavaScript 运行时改写函数行为，检验 S-FBR-19。 |
| FBR-07 | `fiber.spec.ts:122` | update waits for asynchronous recovery | M3 | 已改写 | `crates/cordis/tests/fiber.rs::update_waits_for_asynchronous_recovery`；Clock 推进替代 Promise 定时器，检验 S-FBR-19。 |
| FBR-08 | `fiber.spec.ts:144` | update surfaces a failed recovery to its caller | M3 | 已改写 | `crates/cordis/tests/fiber.rs::update_returns_failed_recovery`；Result 交回恢复失败，检验 S-FBR-21。 |
| FBR-09 | `fiber.spec.ts:156` | update surfaces a failed reload to its caller | M3 | 已改写 | `crates/cordis/tests/fiber.rs::update_returns_failed_reload`；Result 交回重载失败，检验 S-FBR-21。 |
| FBR-10 | `fiber.spec.ts:171` | update does not leak a dropped failure | M3 | 已改写 | `crates/cordis/tests/fiber.rs::dropped_update_waiter_preserves_failure_state`；释放 Future 等待句柄替代 Promise 未处理拒绝，检验 S-FBR-22。 |
| FBR-11 | `fiber.spec.ts:185` | dispose error | M3 | 已改写 | `crates/cordis/tests/fiber.rs::disposal_records_error_and_releases_other_effects`；实际释放序列与错误记录检验 S-FBR-12。 |
| FBR-12 | `fiber.spec.ts:204` | settles a pending fiber to DISPOSED on dispose | M4 | 已改写 | `crates/cordis/tests/service.rs::fbr12_pending_fiber_disposal_reaches_disposed`；类型化依赖声明与 Fiber 句柄表达 Pending 释放，检验 S-FBR-16。 |
| FBR-13 | `fiber.spec.ts:231` | settles a failed fiber to DISPOSED on dispose | M3 | 已改写 | `crates/cordis/tests/fiber.rs::failed_fiber_disposes_with_one_status_transition`；类型化状态事件检验 S-FBR-16。 |
| FBR-14 | `fiber.spec.ts:264` | keeps the active dispose sequence stable | M3 | 已改写 | `crates/cordis/tests/fiber.rs::active_fiber_disposal_status_sequence`；类型化状态事件检验 S-FBR-16 的状态顺序。 |
| FBR-15 | `fiber.spec.ts:292` | update config on wrapped fiber | M3 | 已改写 | `crates/cordis/tests/fiber.rs::cloned_handle_updates_shared_configuration`；Clone 句柄替代 JS 原型包装，检验 S-FBR-19。 |
| FBR-16 | `fiber.spec.ts:312` | restart wrapped fiber | M3 | 已改写 | `crates/cordis/tests/fiber.rs::cloned_handle_restarts_same_fiber`；Clone 句柄共享实例，检验 S-FBR-18。 |
| FBR-17 | `fiber.spec.ts:326` | update config while injected service reloads | M4 | 已改写 | `crates/cordis/tests/service.rs::fbr17_update_during_provider_reload_uses_latest_pair`；组件配置与服务值均类型化，保留新服务和最新配置的组合观测，检验 S-FBR-19、S-SVC-9。 |
| FBR-18 | `fiber.spec.ts:384` | fail on first load | M3 | 已改写 | `crates/cordis/tests/fiber.rs::invalid_initial_configuration_fails_before_apply`；Config::resolve 替代 Standard Schema，检验 S-REG-6。 |
| FBR-19 | `fiber.spec.ts:396` | keep a running component on update | M3 | 已改写 | `crates/cordis/tests/fiber.rs::invalid_update_preserves_active_configuration`；Config::resolve 替代 Standard Schema，检验 S-FBR-19。 |
| FBR-20 | `fiber.spec.ts:413` | keep the issues and their paths | M3 | 已改写 | `crates/cordis/tests/fiber.rs::validation_error_preserves_issues_and_paths`；ValidationError 的类型化 issues 与 path 检验 S-REG-6。 |
| FBR-21 | `fiber.spec.ts:426` | pass the resolved config to listeners | M3 | 已改写 | `crates/cordis/tests/fiber.rs::update_listeners_receive_resolved_configuration`；类型化 resolve 与 RawConfig 检验 S-FBR-19。 |
| FBR-22 | `fiber.spec.ts:448` | keep the source config when a listener handles the update | M3 | 已改写 | `crates/cordis/tests/fiber.rs::handled_update_preserves_source_for_later_restart`；原始配置与解析配置分开保存，检验 S-FBR-20。 |

### UPD：`update.spec.ts`（10 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| UPD-01 | `update.spec.ts:7` | settles concurrent updates on the latest config (inject: %s) | M4 | 已改写 | `crates/cordis/tests/service.rs::upd01_concurrent_updates_with_dependency_coalesce`；以宿主时钟和真实配置记录替代 Promise 门闩与 spy，覆盖带依赖和空依赖，检验 S-FBR-21。 |
| UPD-02 | `update.spec.ts:54` | skips superseded loads before executing the component | M3 | 已改写 | `crates/cordis/tests/update.rs::superseded_loads_are_skipped_before_apply`；确定性队列替代微任务调度，检验 S-FBR-10。 |
| UPD-03 | `update.spec.ts:69` | coalesces updates during unloading | M3 | 已改写 | `crates/cordis/tests/update.rs::updates_coalesce_during_unloading`；Clock 推进替代 Promise 门控，检验 S-FBR-21。 |
| UPD-04 | `update.spec.ts:96` | honors %s while loading even when config is unchanged | M3 | 已改写 | `crates/cordis/tests/update.rs::restart_and_configuration_aba_supersede_loading`；Clock 推进与显式任务检验 S-FBR-18、S-FBR-21。 |
| UPD-05 | `update.spec.ts:121` | cleans up a superseded failure without failing the latest update | M3 | 已改写 | `crates/cordis/tests/update.rs::superseded_failure_preserves_latest_target`；Result 与可推进 Clock 检验 S-FBR-11。 |
| UPD-06 | `update.spec.ts:152` | rejects all waiting updates when the latest load fails | M3 | 已改写 | `crates/cordis/tests/update.rs::concurrent_waiters_receive_latest_failure`；多个 Future 等待者检验 S-FBR-21。 |
| UPD-07 | `update.spec.ts:188` | stops advancing an outdated async generator and collects its final yield | M3 | 已改写 | `crates/cordis/tests/update.rs::cooperative_step_keeps_final_cleanup_and_stops_continuation`；Context::is_current 步骤替代 async generator，检验 T-LIF-7、T-EFF-4。 |
| UPD-08 | `update.spec.ts:212` | waits for dependencies and loads only the latest config | M4 | 已改写 | `crates/cordis/tests/service.rs::upd08_pending_dependency_loads_latest_config`；类型化 ComponentDef 保存最新配置，检验 S-FBR-21、T-COE-4。 |
| UPD-09 | `update.spec.ts:234` | does not restart when a listener accepts an update during loading | M3 | 已改写 | `crates/cordis/tests/update.rs::accepted_update_during_loading_preserves_current_load`；类型化更新瀑布接管配置，检验 S-FBR-20。 |
| UPD-10 | `update.spec.ts:259` | does not load a queued config after disposal | M3 | 已改写 | `crates/cordis/tests/update.rs::disposal_prevents_queued_configuration_load`；Clock 推进替代 Promise 门控，检验 S-FBR-16。 |

### SVC：`service.spec.ts`（8 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| SVC-01 | `service.spec.ts:7` | pending inject | M4 | 已改写 | `crates/cordis/tests/service.rs::svc01_provider_initialization_gates_consumers`；Service.init 改为异步组件初始化，检验 S-SVC-3。 |
| SVC-02 | `service.spec.ts:36` | traceable effect (with inject) | M4 | 已改写 | `crates/cordis/tests/service.rs::svc02_scoped_counter_effect_belongs_to_declaring_consumer`；可追踪代理改为 Scoped.caller 登记真实计数效应，检验 T-CTX-1、DOC-02 §5.3。 |
| SVC-03 | `service.spec.ts:76` | traceable effect (without inject) | M4 | 已改写 | `crates/cordis/tests/service.rs::svc03_root_access_retains_root_effect_ownership`；显式捕获根 Context 表达根访问与效应归属，检验 S-SVC-8、T-CTX-1。 |
| SVC-04 | `service.spec.ts:109` | compare snapshot | M4 | 已改写 | `crates/cordis/tests/service.rs::svc04_service_lifecycle_restores_registration_snapshot`；服务组件与自依赖子组件使用类型化登记，比较服务和组件记录数，检验 S-SVC-1、S-SVC-2。 |
| SVC-05 | `service.spec.ts:127` | multiple injects | M4 | 已改写 | `crates/cordis/tests/service.rs::svc05_multiple_dependencies_and_cycles`；Service 类改为类型化依赖组件，记录实际激活顺序并覆盖依赖环，检验 T-COE-4、T-LIF-10。 |
| SVC-06 | `service.spec.ts:190` | dispose dependents before provider | M4 | 已改写 | `crates/cordis/tests/service.rs::svc06_svc07_svc08_dependency_cleanup_precedes_provider`；defer_async 表达消费者清理，真实读取已提交服务值，检验 T-COE-6、S-FBR-12。 |
| SVC-07 | `service.spec.ts:202` | dispose dependents before provider restarts | M4 | 已改写 | `crates/cordis/tests/service.rs::svc06_svc07_svc08_dependency_cleanup_precedes_provider`；同一用例按 restart 分支核验消费者先清理与重新加载，检验 S-FBR-12、S-FBR-18。 |
| SVC-08 | `service.spec.ts:216` | dispose dependents before isolated provider | M4 | 已改写 | `crates/cordis/tests/service.rs::svc06_svc07_svc08_dependency_cleanup_precedes_provider`；Realm 句柄表达共享隔离标识，检验 S-SVC-5、T-COE-6。 |

### SVD：`service-dispose.spec.ts`（17 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| SVD-01 | `service-dispose.spec.ts:31` | releases an injected binding after providing the same name in another realm (%s) | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd01_releases_dependency_after_same_name_in_another_realm`；类型化服务与 Realm 表达同名自供和已提交依赖，覆盖消费者、提供方、根三种释放，检验 S-SVC-6、S-FBR-12。 |
| SVD-02 | `service-dispose.spec.ts:48` | waits for a disposing consumer that has already left the registry | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd02_withdrawal_waits_for_consumer_after_registry_removal`；真实 Gate 阻塞清理，轮询撤回 Future 核验等待，检验 S-SVC-2 的具体绑定依赖集合。 |
| SVD-03 | `service-dispose.spec.ts:73` | withdraws one binding without waiting for another binding from the same provider | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd03_withdrawal_waits_for_its_own_binding`；两个键由同一组件提供，分别等待清理，检验 S-SVC-2 的逐绑定撤回。 |
| SVD-04 | `service-dispose.spec.ts:95` | retains resources during consumer cleanup (%s) | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd04_retains_resources_through_all_shutdown_orders`；原子资源状态与实际读取记录替代 Service 方法 spy，保留五种释放次序，检验 T-COE-6、S-FBR-12。 |
| SVD-05 | `service-dispose.spec.ts:131` | retains transitive dependencies while a consumer providing another service drains | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd05_retains_transitive_dependencies`；根上下文承载依赖链，真实 Gate 形成传递清理次序，检验 S-FBR-12。 |
| SVD-06 | `service-dispose.spec.ts:156` | retains dependencies through in-flight setup and rollback (failure: %s) | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd06_retains_dependencies_during_setup_and_rollback`；显式 defer 与 Result 表达在途初始化及失败恢复，覆盖成功和失败两支，检验 S-FBR-9、S-SVC-9。 |
| SVD-07 | `service-dispose.spec.ts:184` | releases dependencies after a cleanup error | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd07_releases_dependencies_after_cleanup_error`；Result 错误与内核错误记录替代 throw 和日志 spy，检验 S-FBR-12。 |
| SVD-08 | `service-dispose.spec.ts:199` | keeps provider listeners alive through the final consumer event | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd08_provider_listener_receives_final_consumer_event`；类型化事件记录清理时的最终派发，检验 T-COE-6、S-FBR-12。 |
| SVD-09 | `service-dispose.spec.ts:214` | allows unrelated providers and sibling effects to clean up concurrently | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd09_independent_providers_and_sibling_effects_drain_concurrently`；实际原子状态同时观测独立提供方和同级效应，检验 S-FBR-13。 |
| SVD-10 | `service-dispose.spec.ts:238` | supports composed provider effects (owner shutdown: %s) | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd10_composed_provider_effect_retains_resource`；生成器组合改为 EffectScope.adopt，根作为所属上下文，覆盖局部和所属上下文释放，检验 S-EFF-6、S-SVC-2。 |
| SVD-11 | `service-dispose.spec.ts:271` | keeps an inherited service accessible to a child in a composed effect | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd11_child_in_composed_effect_reads_inherited_provision`；组合效应显式等待子组件释放，检验 S-SVC-2 撤回时的调度让出与继承访问。 |
| SVD-12 | `service-dispose.spec.ts:287` | retains an ancestor injection until its child finishes cleanup | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd12_ancestor_injection_lives_through_child_cleanup`；子组件通过祖先已提交依赖完成异步清理，检验 S-SVC-6、S-FBR-12。 |
| SVD-13 | `service-dispose.spec.ts:307` | waits for a child that explicitly injects its parent service | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd13_parent_provider_waits_for_explicit_child_dependency`；子组件显式注入父组件服务，以真实 Gate 核验清理先后，检验 S-FBR-12。 |
| SVD-14 | `service-dispose.spec.ts:323` | tracks same-name services in separate isolation realms (root shutdown: %s) | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd14_same_name_bindings_in_distinct_realms`；同键不同 Realm 记录独立绑定，覆盖根和提供方释放，检验 T-COE-8、S-FBR-12。 |
| SVD-15 | `service-dispose.spec.ts:355` | withdraws only the old binding without waiting for the next load | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd15_old_withdrawal_finishes_while_new_load_waits`；两个真实 Gate 分别控制旧清理和新加载，检验 S-SVC-2 的绑定身份等待与 S-SVC-9。 |
| SVD-16 | `service-dispose.spec.ts:386` | waits for current and withdrawn bindings during provider disposal (withdrawn first: %s) | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd16_provider_waits_for_current_and_retired_bindings`；两个 Gate 交换释放次序，核验当前及已撤回绑定共同保护提供方资源，检验 S-FBR-12。 |
| SVD-17 | `service-dispose.spec.ts:418` | replaces a binding from the same provider during consumer initialization | M4 | 已改写 | `crates/cordis/tests/service_dispose.rs::svd17_binding_replacement_during_consumer_setup`；真实绑定版本和清理记录表达在途加载取代，检验 S-SVC-9、T-LIF-2。 |

### RFL：`reflect.spec.ts`（6 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| RFL-01 | `reflect.spec.ts:7` | Context.is() | M4 | 不适用 | Context.is 检验 JavaScript 子类对象的运行时识别；DOC-02 §7 将该入口映射为 Rust Context 类型及 Debug 表示。 |
| RFL-02 | `reflect.spec.ts:13` | access check | M4 | 已改写 | `crates/cordis/tests/reflect.rs::rfl02_access_and_registration_contracts`；属性写入能力映射为 Provision.set，类型化 get 执行访问中介，检验 S-SVC-1、S-SVC-7、S-SVC-10。 |
| RFL-03 | `reflect.spec.ts:31` | service injection | M4 | 已改写 | `crates/cordis/tests/reflect.rs::rfl03_service_injection_and_inherited_extension`；属性 mixin 映射为服务键，扩展属性映射为 Context.extend，检验 S-CTX-2、S-SVC-6。 |
| RFL-04 | `reflect.spec.ts:58` | access error | M4 | 已改写 | `crates/cordis/tests/reflect.rs::rfl04_access_error_retains_call_location`；JavaScript 堆栈帧映射为 track_caller 位置链，检验 A-10、S-SVC-7。 |
| RFL-05 | `reflect.spec.ts:99` | service inject leak | M4 | 已改写 | `crates/cordis/tests/reflect.rs::rfl05_disposed_injection_reports_inactive_access`；持有已释放上下文后访问类型化服务，检验 S-SVC-7、S-SVC-9。 |
| RFL-06 | `reflect.spec.ts:109` | does not load a fiber disposed during service notification | M4 | 已改写 | `crates/cordis/tests/reflect.rs::rfl06_disposed_during_notification_keeps_component_unexecuted`；InternalStatus 类型化监听器释放另一个待激活组件，检验 S-SVC-5 与 S-FBR-16。 |

### ISO：`isolate.spec.ts`（6 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| ISO-01 | `isolate.spec.ts:7` | does not expose own provisions across isolation boundaries | M4 | 已改写 | `crates/cordis/tests/isolate.rs::iso01_own_provisions_follow_their_realm`；类型化键与 Realm 句柄替代属性代理及 Symbol，检验 S-SVC-6、T-COE-7。 |
| ISO-02 | `isolate.spec.ts:24` | does not inherit the injected view across isolation boundaries | M4 | 已改写 | `crates/cordis/tests/isolate.rs::iso02_explicit_isolation_stops_ancestor_injection`；同域重新派生仍构成隔离边界，检验 S-SVC-6。 |
| ISO-03 | `isolate.spec.ts:55` | isolated context | M4 | 已改写 | `crates/cordis/tests/isolate.rs::iso03_iso04_independent_and_shared_realms`；独立 Realm 分支核验各域的激活和撤回，检验 T-COE-7、T-COE-8。 |
| ISO-04 | `isolate.spec.ts:106` | shared label | M4 | 已改写 | `crates/cordis/tests/isolate.rs::iso03_iso04_independent_and_shared_realms`；共享 Realm 分支核验两个消费者共享绑定和撤回，检验 T-COE-8。 |
| ISO-05 | `isolate.spec.ts:152` | isolated event | M4 | 已改写 | `crates/cordis/tests/isolate.rs::iso05_service_event_filter_uses_isolation_realm`；服务隐式过滤映射为带类型键域判断的 emit_filtered，检验 S-EVT-3。 |
| ISO-06 | `isolate.spec.ts:172` | keeps own provisions separate from the injected view | M4 | 已改写 | `crates/cordis/tests/isolate.rs::iso06_own_provisions_and_committed_dependency_remain_distinct`；显式 get 区分已提交依赖与两个自供域，检验 S-SVC-6、S-SVC-9。 |

### ICP：`intercept.spec.ts`（7 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| ICP-01 | `intercept.spec.ts:34` | merge context intercepts from outer to inner | M5 | 未开始 | |
| ICP-02 | `intercept.spec.ts:42` | merge injected config under context intercepts | M5 | 未开始 | |
| ICP-03 | `intercept.spec.ts:52` | take injected config from the declaring fiber | M5 | 未开始 | |
| ICP-04 | `intercept.spec.ts:69` | stop looking for injected config at an isolation boundary | M5 | 未开始 | |
| ICP-05 | `intercept.spec.ts:83` | take no injected config for own provisions | M5 | 未开始 | |
| ICP-06 | `intercept.spec.ts:96` | take injected config from the def site | M5 | 未开始 | |
| ICP-07 | `intercept.spec.ts:103` | custom merge | M5 | 未开始 | |

### SHD：`shadow.spec.ts`（8 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| SHD-01 | `shadow.spec.ts:5` | keeps caller metadata separate from the service shadow | M5 | 未开始 | |
| SHD-02 | `shadow.spec.ts:60` | applies the service inject when entered from root | M5 | 未开始 | |
| SHD-03 | `shadow.spec.ts:93` | resolves dependencies through a captured shadow | M5 | 未开始 | |
| SHD-04 | `shadow.spec.ts:157` | resolves same-name own provisions at their definition sites | M5 | 未开始 | |
| SHD-05 | `shadow.spec.ts:195` | keeps unchecked access for services provided on the root ctx | M5 | 未开始 | |
| SHD-06 | `shadow.spec.ts:226` | exposes the caller without preserving shadow for noShadow services | M5 | 未开始 | |
| SHD-07 | `shadow.spec.ts:271` | exposes the caller to callable services | M5 | 未开始 | |
| SHD-08 | `shadow.spec.ts:313` | strips service shadow before creating components | M5 | 未开始 | |

### ASC：`associate.spec.ts`（6 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| ASC-01 | `associate.spec.ts:5` | service injection | M5 | 未开始 | |
| ASC-02 | `associate.spec.ts:30` | property injection | M5 | 未开始 | |
| ASC-03 | `associate.spec.ts:58` | associated type - service injection | M5 | 未开始 | |
| ASC-04 | `associate.spec.ts:114` | associated type - accessor injection | M5 | 未开始 | |
| ASC-05 | `associate.spec.ts:176` | associated access follows the service fiber | M5 | 未开始 | |
| ASC-06 | `associate.spec.ts:235` | inspect | M5 | 未开始 | |

### INV：`invoke.spec.ts`（2 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| INV-01 | `invoke.spec.ts:5` | functional service | M5 | 未开始 | |
| INV-02 | `invoke.spec.ts:62` | uses the service shadow for callable extensions | M5 | 未开始 | |

### LOG：`logger.spec.ts`（11 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| LOG-01 | `logger.spec.ts:18` | keeps the bounded buffer in place and chronological | M5 | 未开始 | |
| LOG-02 | `logger.spec.ts:37` | disposes the exporter that registered the disposer | M5 | 未开始 | |
| LOG-03 | `logger.spec.ts:55` | uses fiber name when called from outside any service | M5 | 未开始 | |
| LOG-04 | `logger.spec.ts:61` | honours explicit name argument | M5 | 未开始 | |
| LOG-05 | `logger.spec.ts:67` | honours intercept name | M5 | 未开始 | |
| LOG-06 | `logger.spec.ts:73` | uses service name when called from inside a Service method (regression) | M5 | 未开始 | |
| LOG-07 | `logger.spec.ts:91` | still lets outer caller intercept override the service-derived name | M5 | 未开始 | |
| LOG-08 | `logger.spec.ts:109` | uses the innermost service name and restores the outer service | M5 | 未开始 | |
| LOG-09 | `logger.spec.ts:140` | uses service name when called from inside [Service.init] (unchanged behaviour) | M5 | 未开始 | |
| LOG-10 | `logger.spec.ts:156` | logs an error whose errors array is empty | M5 | 未开始 | |
| LOG-11 | `logger.spec.ts:165` | flattens an aggregate error into its members | M5 | 未开始 | |

### RPL：`replace.spec.ts`（16 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| RPL-01 | `replace.spec.ts:7` | swaps the implementation behind a fiber in place | M5 | 未开始 | |
| RPL-02 | `replace.spec.ts:29` | emits internal/replace for each moved fiber | M5 | 未开始 | |
| RPL-03 | `replace.spec.ts:46` | validates the config with the new schema | M5 | 未开始 | |
| RPL-04 | `replace.spec.ts:60` | takes the dependencies of the new component | M5 | 未开始 | |
| RPL-05 | `replace.spec.ts:79` | starts a pending fiber when the new component drops the missing dependency | M5 | 未开始 | |
| RPL-06 | `replace.spec.ts:92` | starts a pending fiber when the new component injects an available service | M5 | 未开始 | |
| RPL-07 | `replace.spec.ts:106` | stops depending on a dependency that the new component drops | M5 | 未开始 | |
| RPL-08 | `replace.spec.ts:121` | reloads a consumer of a replaced service | M5 | 未开始 | |
| RPL-09 | `replace.spec.ts:145` | merges the fibers into a registered component | M5 | 未开始 | |
| RPL-10 | `replace.spec.ts:162` | restarts a failed fiber | M5 | 未开始 | |
| RPL-11 | `replace.spec.ts:176` | leaves a fiber failed when the new component fails to apply | M5 | 未开始 | |
| RPL-12 | `replace.spec.ts:186` | forwards the old component to the new one | M5 | 未开始 | |
| RPL-13 | `replace.spec.ts:208` | forwards a component that is not registered | M5 | 未开始 | |
| RPL-14 | `replace.spec.ts:220` | follows a chain of replacements | M5 | 未开始 | |
| RPL-15 | `replace.spec.ts:234` | switches back to a replaced component | M5 | 未开始 | |
| RPL-16 | `replace.spec.ts:248` | unregisters the new component when its last fiber is disposed | M5 | 未开始 | |

### DEC：`decorator.spec.ts`（1 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| DEC-01 | `decorator.spec.ts:7` | @Inject on class method | M6 | 未开始 | |
