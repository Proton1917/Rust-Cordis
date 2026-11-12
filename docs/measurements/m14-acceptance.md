# M14 运行时验收与测量

本次验收使用 macOS Apple Silicon、14 个逻辑处理器、48 GiB 内存。核心、宿主与开发工具采用 Rust 1.96.0；跨编译器使用方采用 Rust 1.85.0。固定压力场景与直接调用基准采用 release 构建，其他功能场景沿用各测试的构建配置。

## 行为场景

| 场景 | 实际观察 | 自动化入口与证据 |
|---|---|---|
| AC-1 | 8 个独立插件库、100 个实例在 6139 ms 内全部 Active | `acceptance::eight_independent_libraries_and_one_hundred_instances`；[启动记录](m14-startup.json) |
| AC-2 | 持续计数期间，kv-mem → kv-file → kv-mem 0.2 后计数分别达到 29、50；counter 自动重建，idle 保持纪元，旧库租约归零 | `runtime::migration::continuous_counter_migrates_rolls_back_rejects_and_restores_across_compilers`；[迁移记录](m14-kv-migration.json) |
| AC-3 | 真实 apply 失败的 KV 候选自动回滚并继承原状态；watch 对同一失败来源保持已确认版本 | 上述 migration 场景；`watch::watch_replaces_rolls_back_suppresses_same_candidate_and_reloads` |
| AC-4 | 提供 kv@2 的真实库在 SW-4 拒绝，store、counter 和同组 idle 的 context 保持 | 上述 migration 场景 |
| AC-5 | SIGTERM 保存计数 109，重启读取 113，消费后的状态文件进入 consumed | 上述 migration 场景 |
| AC-6 | 真实验证印记准入；缺印记和改写字节候选在 SW-2 拒绝；已验证 kv-file 经阈值重启后保持 `[9,7,5]` | `runtime::gate::verified_runtime_install_replace_and_threshold_restart_use_real_receipts` |
| AC-7、AU-1 | 服务 panic、apply panic、段错误、栈溢出、abort、进程退出和死循环按 ABI 或分组边界处理；其他分组继续运行 | `cordis-host/tests/native.rs` 的 panic、fault 与 exit 场景；`cordis-abi/tests/boundary.rs` 的 code 5 场景 |
| AC-8 | Rust 1.96 counter 替换为 Rust 1.85 counter 后继续到 107，build_info 分别报告相应编译器 | migration 场景；原生宿主两种调用方式的真实 1.85 使用方 |
| AC-9、AU-10 | 8 个实际线程持续调用，固定 50 次替换；每轮实现版本和状态 137 正确，第 21、42 次执行有状态宿主重启 | `stress::eight_threads_survive_fifty_native_replacements_with_stateful_restarts`；[全部 50 轮样本](m14-native-replacement.json) |
| AU-2 | 超大帧、畸形 CBOR、洪泛和超时进程被隔离，独立正常进程保持响应 | `cordis-supervisor::process::malformed_frames_timeouts_and_flood_leave_other_processes_responsive` |
| AU-8 | 核心由 cordis、supervisor、runtime、up、process、types 组成；产品依赖遵循 Q0，安全审计通过 | `target/m13-core-dependencies.txt`；M14 工作区 `unsafe_is_localized_and_documents_its_contract`，产品依赖声明沿用 M13 |
| AU-9 | 同组直接调用达到预定中位数比和最大值比阈值 | [直接调用原始样本](m14-direct-call.json)；`benchmarks/direct-call/` |
| AU-11 | launchd 在 SIGKILL 后拉起核心，旧 Host 退出；检查点 73 覆盖配置初值 29，恢复用时 9938 ms；临时服务已卸载 | `acceptance::launchd_restores_core_and_checkpoint_after_core_exit`；[恢复记录](m14-core-recovery.json) |
| AU-16 | 逐方法管理授权生效；install、replace、rollback、replace 及强制退出后的已确认版本恢复通过 | `runtime::management_and_checkpoints_survive_core_restart` |
| AU-18 | 两个真实 kv-file 在 tenant-a/b 对同名键分别保持 `[1,2]`、`[9,8]`，flush 后文件内容对应；同组、跨组、进程提供方均收到 namespace 与 marker=17，读写值为 `[4,2,9]` | `runtime::isolation` 的两条用例；同组 typed=true，跨组和进程 typed=false；派生域、Scoped 缓存与父子清理由 native composition 和 supervisor derived_scopes 场景覆盖 |
| README 首个插件 | 从空目录按默认固定 Git SDK 创建项目和插件，编译并进入 Active | `acceptance::readme_first_plugin_builds_from_pinned_git_sdk` |

## 可替换性指标

RM-1 同时报告代码行和总行。统计范围是插件实现、实际读取的接口定义及对应契约套件；使用 cloc 2.10 的 Rust 注释规则，逐文件计数。

| 插件 | RM-1 代码行 / 总行 | 接口与契约范围 |
|---|---:|---|
| kv-mem | 178 / 225 | kv-api、kv-contract、cordis-contract |
| kv-file | 286 / 347 | kv-api、kv-contract、cordis-contract |
| counter | 186 / 230 | kv-api、kv-contract、cordis-contract |
| console-log | 516 / 578 | 使用的 SDK builtin 接口与 cordis-types；契约覆盖来自运行时真实日志测试 |
| watch | 835 / 925 | 使用的 SDK builtin 接口与 cordis-types；契约覆盖来自运行时真实监视测试 |

| 指标 | 结果 | 口径 |
|---|---|---|
| RM-2 重编译范围 | 1 个 crate：load_one | 修改 load-one 激活消息后，记录实际编译的工作区成员 |
| RM-3 受影响实例数 | 中位数 3，最大值 4 | 50 次原生替换；第 21、42 次阈值重启额外重建 idle |
| RM-4 替换停顿 | 中位数 241 ms，最大值 661 ms | 串行事务执行者开始准备候选，到全部受影响实例 Active；按 ADR-0035 扣除前一确认期 |
| 完整管理 RPC | 中位数 260.5165 ms，最大值 682.222 ms | 进程插件在 get/invoke 周围计时，包含确认等待 |
| 外部驱动耗时 | 中位数 334.1625 ms，最大值 745.882792 ms | 包含命令文件收发与外部状态观察 |
| RM-5 契约验证 | kv-mem 2723 ms，kv-file 1734 ms | M13 实际独立核心的两阶段 kv-contract，各 8 条断言；复用 `target/verify-live/kv-mem.log`、`kv-file.log` |
| RM-6 常驻内存增量 | 全部运行进程 118.4 KiB/次；核心 27.84 KiB/次 | 50 次后 RSS 分别从 19856→25776 KiB、8080→9472 KiB，差额除以 50 |
| RM-9 同组直接调用 | 基线中位数 19.66792 ns，当前 18.62083 ns；中位数比 0.946762、两组最大值比 0.721550 | 同一 provider、同为 Rust 1.85 release 的 consumer；M9 基线 dc93da1；固定 9 轮，每轮预热 10000 次、测量 100000 次完整调用 |
| RM-10 核心恢复 | 9938 ms | launchd 托管核心收到 SIGKILL，到全部插件恢复 Active |

两次阈值重启前后，原生宿主 RSS 分别从 13664 降至 5616 KiB、13712 降至 5632 KiB。每轮记录包含实际 PID、全部进程 RSS、8 个线程的读取结果及 idle 纪元。

直接调用基准预先规定中位数比 ≤1.05、两组最大值比 ≤1.15。9 轮达到这两个标准；两组样本随轮次共同下降，结果用于本次相同条件的退化判定。全样本保留在 JSON，逐轮比值也一并记录。

## 执行与证据

功能命令由 `cargo test -p cordis-runtime --test runtime`、`--test acceptance`、`--test stress` 按具体测试名执行；直接调用复现入口和环境见 `benchmarks/direct-call/README.md`。本阶段成功的定向验收由工作区退出检查复用，固定测量按原定轮数结束。

| 验收 | 日志与原始目录 |
|---|---|
| AC-1、RM-2 | `target/m14-ac1.log`；`target/acceptance-evidence/ac1-77490-1794509227162662000` |
| KV 迁移、回滚、主版本与编译器切换 | `target/m14-kv-migration-observation-fixed.log`；`target/runtime-m12/cases/kv-migration-93479` |
| KV 隔离与 Scoped | `target/m14-kv-isolation.log`；`target/runtime-m12/cases/real-kv-domains-95044`、`scoped-kv-{same,cross,process}-95044` |
| AC-9、RM-3/4/6 | `target/m14-ac9.log`；`target/acceptance-evidence/ac9-96730-1794514168429967000` |
| README 与 AU-11 | `target/m14-readme-recovery.log`；`target/acceptance-evidence/readme-85318-1794510936167906000`、`au11-85318-1794510936167911000` |
| RM-9 | `target/m14-benchmark/latency-run.log`；`target/m14-benchmark/build/native-run-91905/direct-call-measurements.json` |
| RM-1 | `target/m14-rm1-{kv-mem,kv-file,counter,console-log,watch}.json` |

JSON 测量文件随仓库保存；`target/` 中保留运行目录和完整日志供实现阶段核查。
