# M21 调用时延与替换停顿

测量使用 Apple M4 Pro、14 个逻辑 CPU、48 GiB 内存，系统记录为 macOS 27.2 arm64。Rust 主程序、原生 RPC 调用方和提供方采用 Rust 1.96.0 release；直接调用比较的两个 consumer 与同一 provider 采用 Rust 1.85.0 release。常规桌面软件和模型会话保持运行状态。

原始 JSON 位于 raw/，构建日志位于 builds/，机器可读汇总为 summary.json。数值采用实际保留的全部样本，报告中的路径、PID、描述符及状态来自对应运行。

## RM-7 跨进程调用

调用方是独立 cordis-host 分组中的原生 Rust latency-probe，三种提供方分别位于另一个分组。每次计时从 Service<Kv>::get 入口前开始，到返回 Vec 的释放完成后结束，包含调用方 SDK/ABI、Host/Core/提供方的 IPC 往返、服务处理、解码、复制和结果释放。服务句柄取得、初始写入及一次正确性读取在采样前完成。

每个方向固定10000次读取，键为 cordis-measurement，值为0至63组成的64字节数组。中位数取排序后的两个中间值平均数，99分位使用nearest-rank。wall_ns覆盖完整采样循环，原始sample_ns保留执行顺序。

| 方向 | 样本数 | 中位数/µs | 99分位/µs | 采样循环/s |
|---|---:|---:|---:|---:|
| 原生Rust→原生Rust，跨分组 | 10000 | 124.833 | 237.750 | 1.300141 |
| 原生Rust→Python | 10000 | 116.875 | 146.125 | 1.200218 |
| 原生Rust→TypeScript | 10000 | 81.375 | 175.084 | 0.866478 |

三个原始文件的形成时间分别为2026-11-13 12:53:37、12:53:39、12:53:41 UTC。脚本提供方使用当时的conda base Python3.12.12与Homebrew Node安装目录node/26.13.0_2；原生调用方的实际描述符记录Rust1.96.0。每个文件保存调用方与提供方的不同PID及完整运行快照。结果对应当前宿主和SDK路径。

## RM-8 替换停顿

每个方向执行一次，业务状态为同一64字节值，confirm_ms为20。pause_ms沿ADR-0035，从串行事务开始准备候选计至全部受影响实例Active；完整管理调用由真实Rust管理客户端在get/invoke周围计时；外部驱动从命令文件发布计至回复文件被读到。原生结果取last_swap，进程结果取last_replace。

| 实际替换 | 插口 | 次数 | pause/ms | 管理RPC/ms | 外部驱动/ms |
|---|---|---:|---:|---:|---:|
| Rust kv-mem→kv-file | native | 1 | 215 | 259.048 | 342.219 |
| Python→TypeScript | process | 1 | 362 | 458.916 | 471.358 |
| TypeScript→Python | process | 1 | 126 | 181.729 | 241.936 |

三次均观察到新的安装引用、提供方context改变、依赖使用方重新Active，原64字节值通过真实get读回。结果文件形成于12:58:40、13:01:41、13:03:30 UTC；完整来源、状态与替换报告保存在对应原始JSON。表格列出该次观测，进程行同时明确两种实现之间的替换方向。

## RM-9 同组直接调用

M9提交dc93da1沿用为第二阶段的可复现基线。baseline/current使用同一consumer.rs、Rust1.85.0、相同release配置与同一个provider；宿主和驱动使用固定核心工具链。每种实现固定9轮，每轮预热10000次、计量100000次完整Counter::value调用，逐轮交替两实现的顺序。预定上限保持中位数比1.05、最大值比1.15。

| 运行 | 基线中位数/ns | 正式current中位数/ns | 中位数比 | 最大值比 | 判据结果 |
|---|---:|---:|---:|---:|---|
| 首次窗口 | 19.14959 | 20.62625 | 1.07711183 | 0.89729184 | 中位数比超过1.05 |
| 统一停载窗口 | 20.22250 | 19.36792 | 0.95774113 | 0.90793525 | 两项均满足 |

首次结果于13:06:02 UTC形成。root转达设计方45fa107f的时段证据：其全量测试持续至13:10 UTC，首次前4轮与重负载重叠。原18个样本及原判据结果完整保存在rm9-initial.json。

root于13:39:06 UTC确认Cargo、rustc、测试及benchmark任务清单为空，并协调root、frontend和设计方停载。唯一一次原正式实现重测于13:39:54 UTC启动，结果于13:39:56 UTC形成；全部18个样本保存在rm9-coordinated-idle.json。两个窗口使用相同的baseline、provider、host和55981-1目录中的原正式current快照，优化候选留存于独立目录。

停载重测完成预定轮数后结束。此次原正式实现满足RM-9的原定判据。RM-7、RM-8保留首次任务窗口的本机观测，统一停载重测记录对应RM-9。

两次测量的第1–4轮均偏高，停载窗口也从约36–40ns下降至14–15ns。两个窗口的中位数比相差约12%，大于5%的判据宽度，说明短预热、短采样窗口具有可见变动。本阶段沿用原方法和全部样本；预热长度及交替多次取中位的方法留作后续独立研究。ADR-0045静态分派候选保持暂停，当前SDK与Service实现保持原值。

## 原始文件与构建

- raw/rm7-rust-to-rust.json、rm7-rust-to-python.json、rm7-rust-to-typescript.json：每文件10000个原始时延及运行快照。
- raw/rm8-native.json、rm8-python-typescript.json、rm8-typescript-python.json：三次完整替换记录和实际前后状态。
- raw/rm9-initial.json、rm9-coordinated-idle.json：两个窗口各18个完整样本、编译器描述符、阈值和判定。
- builds/：主程序、latency-probe、管理夹具、baseline/current consumer、provider及直接调用驱动的构建记录。

AU-9采用原正式实现的统一停载结果；首次窗口结果继续保留。总体场景的对应关系见同级目录外的 M21 验收记录。
