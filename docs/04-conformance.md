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
| DSP-01 | `dispose.spec.ts:7` | dispose by component | M3 | 未开始 | |
| DSP-02 | `dispose.spec.ts:23` | dispose manually | M1 | 未开始 | |
| DSP-03 | `dispose.spec.ts:37` | yield dispose | M1 | 未开始 | |
| DSP-04 | `dispose.spec.ts:76` | async return 1 | M3 | 未开始 | |
| DSP-05 | `dispose.spec.ts:90` | async return 2 | M3 | 未开始 | |
| DSP-06 | `dispose.spec.ts:103` | async yield 1 | M3 | 未开始 | |
| DSP-07 | `dispose.spec.ts:123` | async yield 2 (aborted) | M3 | 未开始 | |
| DSP-08 | `dispose.spec.ts:143` | async yield 3 (aborted) | M3 | 未开始 | |
| DSP-09 | `dispose.spec.ts:165` | async yield (aborted, finally) | M3 | 未开始 | |
| DSP-10 | `dispose.spec.ts:185` | async yield 4 (await dispose) | M3 | 未开始 | |
| DSP-11 | `dispose.spec.ts:205` | return with error | M1 | 未开始 | |
| DSP-12 | `dispose.spec.ts:217` | yield with error | M1 | 未开始 | |
| DSP-13 | `dispose.spec.ts:230` | async return with error | M3 | 未开始 | |
| DSP-14 | `dispose.spec.ts:242` | async yield with error | M3 | 未开始 | |
| DSP-15 | `dispose.spec.ts:261` | async yield invalid effect | M3 | 未开始 | |

### EVT：`events.spec.ts`（31 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| EVT-01 | `events.spec.ts:18` | deprecated dispatch returns bound callbacks that respect later removal | M2 | 未开始 | |
| EVT-02 | `events.spec.ts:35` | deprecated waterfall dispatch continues past a removed callback | M2 | 未开始 | |
| EVT-03 | `events.spec.ts:49` | supports symbol event names across dispatch modes | M2 | 未开始 | |
| EVT-04 | `events.spec.ts:66` | supports symbol event names with ctx.once() | M2 | 未开始 | |
| EVT-05 | `events.spec.ts:77` | treats prototype property names as ordinary events | M2 | 未开始 | |
| EVT-06 | `events.spec.ts:96` | removes empty event buckets after disposal | M2 | 未开始 | |
| EVT-07 | `events.spec.ts:108` | ctx.on() | M2 | 未开始 | |
| EVT-08 | `events.spec.ts:121` | ctx.once() | M2 | 未开始 | |
| EVT-09 | `events.spec.ts:134` | ctx.once() runs once when an earlier listener emits recursively | M2 | 未开始 | |
| EVT-10 | `events.spec.ts:149` | ctx.once() runs once across concurrent serial dispatches | M2 | 未开始 | |
| EVT-11 | `events.spec.ts:163` | %s skips listeners removed during dispatch | M2 | 未开始 | |
| EVT-12 | `events.spec.ts:177` | %s defers added listeners until the next dispatch | M2 | 未开始 | |
| EVT-13 | `events.spec.ts:188` | skips a listener disposed by an earlier component during dispatch | M3 | 未开始 | |
| EVT-14 | `events.spec.ts:200` | waterfall continues past a removed listener | M2 | 未开始 | |
| EVT-15 | `events.spec.ts:219` | waterfall defers added listeners until the next dispatch | M2 | 未开始 | |
| EVT-16 | `events.spec.ts:233` | waterfall continues after a recursive dispatch has consumed a once listener | M2 | 未开始 | |
| EVT-17 | `events.spec.ts:251` | internal/update continues past a removed fiber listener | M3 | 未开始 | |
| EVT-18 | `events.spec.ts:280` | ctx.parallel() | M2 | 未开始 | |
| EVT-19 | `events.spec.ts:311` | ctx.parallel() labels symbol events | M2 | 未开始 | |
| EVT-20 | `events.spec.ts:322` | ctx.emit() | M2 | 未开始 | |
| EVT-21 | `events.spec.ts:341` | ctx.serial() | M2 | 未开始 | |
| EVT-22 | `events.spec.ts:360` | ctx.bail() | M2 | 未开始 | |
| EVT-23 | `events.spec.ts:379` | ctx.waterfall() | M2 | 未开始 | |
| EVT-24 | `events.spec.ts:407` | ctx.waterfall() rejects duplicate next() | M2 | 未开始 | |
| EVT-25 | `events.spec.ts:421` | ctx.waterfall() rejects continuations from outer frames | M2 | 未开始 | |
| EVT-26 | `events.spec.ts:442` | ctx.waterfall() rejects duplicate next() after awaiting | M2 | 未开始 | |
| EVT-27 | `events.spec.ts:455` | ctx.waterfall() supports nested async calls | M2 | 未开始 | |
| EVT-28 | `events.spec.ts:473` | does not accumulate across reloads | M3 | 未开始 | |
| EVT-29 | `events.spec.ts:490` | can be disposed explicitly | M3 | 未开始 | |
| EVT-30 | `events.spec.ts:507` | runs prepended listeners first | M3 | 未开始 | |
| EVT-31 | `events.spec.ts:534` | global listeners are not tied to a child fiber reload | M3 | 未开始 | |

### CMP：`component.spec.ts`（11 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| CMP-01 | `component.spec.ts:8` | apply functional component | M3 | 未开始 | |
| CMP-02 | `component.spec.ts:18` | apply object component | M3 | 未开始 | |
| CMP-03 | `component.spec.ts:29` | apply component through deprecated ctx.plugin() | M3 | 未开始 | |
| CMP-04 | `component.spec.ts:40` | apply invalid component | M3 | 未开始 | |
| CMP-05 | `component.spec.ts:47` | inactive context | M3 | 未开始 | |
| CMP-06 | `component.spec.ts:61` | context inspect | M3 | 未开始 | |
| CMP-07 | `component.spec.ts:88` | ctx.registry | M3 | 未开始 | |
| CMP-08 | `component.spec.ts:97` | nested components | M3 | 未开始 | |
| CMP-09 | `component.spec.ts:134` | compare snapshot | M3 | 未开始 | |
| CMP-10 | `component.spec.ts:155` | root dispose | M3 | 未开始 | |
| CMP-11 | `component.spec.ts:175` | Service.init | M4 | 未开始 | |

### FBR：`fiber.spec.ts`（22 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| FBR-01 | `fiber.spec.ts:7` | inertia lock 1 | M4 | 未开始 | |
| FBR-02 | `fiber.spec.ts:27` | inertia lock 2 | M4 | 未开始 | |
| FBR-03 | `fiber.spec.ts:49` | inertia lock 3 | M4 | 未开始 | |
| FBR-04 | `fiber.spec.ts:71` | component error | M3 | 未开始 | |
| FBR-05 | `fiber.spec.ts:93` | failed fiber does not re-enter on dependency refresh | M4 | 未开始 | |
| FBR-06 | `fiber.spec.ts:108` | update recovers a failed fiber | M4 | 未开始 | |
| FBR-07 | `fiber.spec.ts:122` | update waits for asynchronous recovery | M3 | 未开始 | |
| FBR-08 | `fiber.spec.ts:144` | update surfaces a failed recovery to its caller | M3 | 未开始 | |
| FBR-09 | `fiber.spec.ts:156` | update surfaces a failed reload to its caller | M3 | 未开始 | |
| FBR-10 | `fiber.spec.ts:171` | update does not leak a dropped failure | M3 | 未开始 | |
| FBR-11 | `fiber.spec.ts:185` | dispose error | M3 | 未开始 | |
| FBR-12 | `fiber.spec.ts:204` | settles a pending fiber to DISPOSED on dispose | M4 | 未开始 | |
| FBR-13 | `fiber.spec.ts:231` | settles a failed fiber to DISPOSED on dispose | M3 | 未开始 | |
| FBR-14 | `fiber.spec.ts:264` | keeps the active dispose sequence stable | M3 | 未开始 | |
| FBR-15 | `fiber.spec.ts:292` | update config on wrapped fiber | M3 | 未开始 | |
| FBR-16 | `fiber.spec.ts:312` | restart wrapped fiber | M3 | 未开始 | |
| FBR-17 | `fiber.spec.ts:326` | update config while injected service reloads | M4 | 未开始 | |
| FBR-18 | `fiber.spec.ts:384` | fail on first load | M3 | 未开始 | |
| FBR-19 | `fiber.spec.ts:396` | keep a running component on update | M3 | 未开始 | |
| FBR-20 | `fiber.spec.ts:413` | keep the issues and their paths | M3 | 未开始 | |
| FBR-21 | `fiber.spec.ts:426` | pass the resolved config to listeners | M3 | 未开始 | |
| FBR-22 | `fiber.spec.ts:448` | keep the source config when a listener handles the update | M3 | 未开始 | |

### UPD：`update.spec.ts`（10 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| UPD-01 | `update.spec.ts:7` | settles concurrent updates on the latest config (inject: %s) | M4 | 未开始 | |
| UPD-02 | `update.spec.ts:54` | skips superseded loads before executing the component | M3 | 未开始 | |
| UPD-03 | `update.spec.ts:69` | coalesces updates during unloading | M3 | 未开始 | |
| UPD-04 | `update.spec.ts:96` | honors %s while loading even when config is unchanged | M3 | 未开始 | |
| UPD-05 | `update.spec.ts:121` | cleans up a superseded failure without failing the latest update | M3 | 未开始 | |
| UPD-06 | `update.spec.ts:152` | rejects all waiting updates when the latest load fails | M3 | 未开始 | |
| UPD-07 | `update.spec.ts:188` | stops advancing an outdated async generator and collects its final yield | M3 | 未开始 | |
| UPD-08 | `update.spec.ts:212` | waits for dependencies and loads only the latest config | M4 | 未开始 | |
| UPD-09 | `update.spec.ts:234` | does not restart when a listener accepts an update during loading | M3 | 未开始 | |
| UPD-10 | `update.spec.ts:259` | does not load a queued config after disposal | M3 | 未开始 | |

### SVC：`service.spec.ts`（8 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| SVC-01 | `service.spec.ts:7` | pending inject | M4 | 未开始 | |
| SVC-02 | `service.spec.ts:36` | traceable effect (with inject) | M4 | 未开始 | |
| SVC-03 | `service.spec.ts:76` | traceable effect (without inject) | M4 | 未开始 | |
| SVC-04 | `service.spec.ts:109` | compare snapshot | M4 | 未开始 | |
| SVC-05 | `service.spec.ts:127` | multiple injects | M4 | 未开始 | |
| SVC-06 | `service.spec.ts:190` | dispose dependents before provider | M4 | 未开始 | |
| SVC-07 | `service.spec.ts:202` | dispose dependents before provider restarts | M4 | 未开始 | |
| SVC-08 | `service.spec.ts:216` | dispose dependents before isolated provider | M4 | 未开始 | |

### SVD：`service-dispose.spec.ts`（17 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| SVD-01 | `service-dispose.spec.ts:31` | releases an injected binding after providing the same name in another realm (%s) | M4 | 未开始 | |
| SVD-02 | `service-dispose.spec.ts:48` | waits for a disposing consumer that has already left the registry | M4 | 未开始 | |
| SVD-03 | `service-dispose.spec.ts:73` | withdraws one binding without waiting for another binding from the same provider | M4 | 未开始 | |
| SVD-04 | `service-dispose.spec.ts:95` | retains resources during consumer cleanup (%s) | M4 | 未开始 | |
| SVD-05 | `service-dispose.spec.ts:131` | retains transitive dependencies while a consumer providing another service drains | M4 | 未开始 | |
| SVD-06 | `service-dispose.spec.ts:156` | retains dependencies through in-flight setup and rollback (failure: %s) | M4 | 未开始 | |
| SVD-07 | `service-dispose.spec.ts:184` | releases dependencies after a cleanup error | M4 | 未开始 | |
| SVD-08 | `service-dispose.spec.ts:199` | keeps provider listeners alive through the final consumer event | M4 | 未开始 | |
| SVD-09 | `service-dispose.spec.ts:214` | allows unrelated providers and sibling effects to clean up concurrently | M4 | 未开始 | |
| SVD-10 | `service-dispose.spec.ts:238` | supports composed provider effects (owner shutdown: %s) | M4 | 未开始 | |
| SVD-11 | `service-dispose.spec.ts:271` | keeps an inherited service accessible to a child in a composed effect | M4 | 未开始 | |
| SVD-12 | `service-dispose.spec.ts:287` | retains an ancestor injection until its child finishes cleanup | M4 | 未开始 | |
| SVD-13 | `service-dispose.spec.ts:307` | waits for a child that explicitly injects its parent service | M4 | 未开始 | |
| SVD-14 | `service-dispose.spec.ts:323` | tracks same-name services in separate isolation realms (root shutdown: %s) | M4 | 未开始 | |
| SVD-15 | `service-dispose.spec.ts:355` | withdraws only the old binding without waiting for the next load | M4 | 未开始 | |
| SVD-16 | `service-dispose.spec.ts:386` | waits for current and withdrawn bindings during provider disposal (withdrawn first: %s) | M4 | 未开始 | |
| SVD-17 | `service-dispose.spec.ts:418` | replaces a binding from the same provider during consumer initialization | M4 | 未开始 | |

### RFL：`reflect.spec.ts`（6 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| RFL-01 | `reflect.spec.ts:7` | Context.is() | M4 | 未开始 | |
| RFL-02 | `reflect.spec.ts:13` | access check | M4 | 未开始 | |
| RFL-03 | `reflect.spec.ts:31` | service injection | M4 | 未开始 | |
| RFL-04 | `reflect.spec.ts:58` | access error | M4 | 未开始 | |
| RFL-05 | `reflect.spec.ts:99` | service inject leak | M4 | 未开始 | |
| RFL-06 | `reflect.spec.ts:109` | does not load a fiber disposed during service notification | M4 | 未开始 | |

### ISO：`isolate.spec.ts`（6 条）

| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |
|---|---|---|---|---|---|
| ISO-01 | `isolate.spec.ts:7` | does not expose own provisions across isolation boundaries | M4 | 未开始 | |
| ISO-02 | `isolate.spec.ts:24` | does not inherit the injected view across isolation boundaries | M4 | 未开始 | |
| ISO-03 | `isolate.spec.ts:55` | isolated context | M4 | 未开始 | |
| ISO-04 | `isolate.spec.ts:106` | shared label | M4 | 未开始 | |
| ISO-05 | `isolate.spec.ts:152` | isolated event | M4 | 未开始 | |
| ISO-06 | `isolate.spec.ts:172` | keeps own provisions separate from the injected view | M4 | 未开始 | |

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
