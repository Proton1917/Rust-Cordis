# 跨进程调用与替换测量

本目录的驱动分别测量跨进程 KV 调用（RM-7）、替换停顿（RM-8）和同组原生调用（RM-9）。每个子命令对应一种测量。以下示例从仓库根目录执行，使用共享编译目录。

```sh
build_root="${CARGO_TARGET_DIR:-$PWD/target}"
cargo run --manifest-path benchmarks/multilanguage/driver/Cargo.toml \
  --target-dir "$build_root/shared" -- rpc
```

## 跨进程读取

RM-7 的调用方是独立 cordis-host 分组中的原生 Rust 插件。三个方向分别调用另一分组的 Rust、Python、TypeScript KV 提供方，每向固定 10000 次。准备阶段写入相同的 64 字节值并执行一次正确性读取；每次计时从 Service<Kv>::get 入口前开始，到返回值释放后结束，覆盖 SDK/ABI、Host↔Core↔提供方通信、提供方处理以及回程解码、复制和释放。报告保存全部 sample_ns、中位数、99 分位和总循环时间。

### 两版运行时配对测量

`benchmarks.deployment.rpc` 使用同一份已编译的探针和 KV 提供方，分别运行两个构建目录中的 `cordis`、`cordis-host`。三个载荷大小为 64 B、1 MiB、4 MiB；提供方与使用方分别位于 `store`、`consumer` 宿主分组。以下命令从仓库根目录执行，输入目录均为已完成构建的产物目录：

```sh
python -m benchmarks.deployment.rpc \
  /path/to/baseline/release /path/to/candidate/release \
  /path/to/probe/release/libcordis_latency_probe.so \
  /path/to/candidate/release/libkv_mem.so /path/to/evidence \
  --pairs 5 --warmup-pairs 1 \
  --samples 2000 --warmup 200 --large-samples 128 --large-warmup 8
```

macOS 使用 `.dylib`。探针通过 `benchmarks/multilanguage/probe/Cargo.toml` 构建；`bytes` 配置在探针内生成固定字节序列，已有 `value` 数组配置仍可使用，两种配置分别使用。两版运行时需要支持所选原生插件的 ABI。

`inputs.json` 保存所有程序及共享插件的 SHA256、实际样本数和环境。每轮独立启动并交替执行版本；预热对标记为 `warmup_pair`，正式样本标记为 `formal`。`rpc.jsonl` 和逐轮 JSON 均包含全部原始时延、每轮吞吐和分位数。`--size` 可单独选择载荷，`--timeout` 控制每次部署的等待期限，`--keep-runs` 保留完整部署目录及日志。失败现场始终保留，停机时回收该部署独立会话中的宿主进程。

`wall_ns` 测量探针的串行读取循环，包含单次计时和时延数组写入；准备写入、正确性检查、预热以及进程启动分别位于该窗口之外。其吞吐反映单个串行使用方的完整调用路径。Linux 的 RSS/PSS 在结果发布后按进程采样，`peak_rss` 来自每个进程的 `VmHWM`，覆盖启动、数据准备、测量和结果发布的整个进程生命期。各进程的峰值可能发生在不同时间。这里的进程内存数据与 DTO 微基准中的申请堆字节分别记录。

`--concurrency 4` 使用四个系统线程共享已绑定的服务句柄。各线程完成采样数组准备后同时开始，配置中的总调用数分配给全部线程。并发模式逐次检查完整返回字节，单次延迟包含检查与结果释放；总时间从统一放行开始，到最后一个线程完成结束。线程创建、回收和样本合并位于总时间窗口之外。串行与并发模式分别做版本配对，`inputs.json` 和每轮报告保存实际并发数。

## 替换停顿

RM-8 每条指定转换执行一次。原生测量使用实际 kv-mem→kv-file：

```sh
cargo run --manifest-path benchmarks/multilanguage/driver/Cargo.toml \
  --target-dir "$build_root/shared" -- replace --label native \
  --provider "$build_root/shared/release/libkv_mem.dylib" \
  --candidate "$build_root/shared/release/libkv_file.dylib"
```

Python→TypeScript、TypeScript→Python 使用同样的 replace 入口，传入各自完整交付目录。驱动记录实际方向，核对新安装引用、提供方上下文变化、依赖使用方恢复及原 64 字节值；pause_ms、管理调用时间与外部观察时间分别保存。confirm_ms 为 20。

## 同组调用

同组方法使用 [direct 驱动](../direct-call/README.md)。其计时范围从已绑定服务的调用开始，结果与跨进程读取分开解释。[历史样本与负载条件](../../docs/measurements/m21-performance/README.md)

## 工具与样本目录

`prepare` 模式集中完成程序、依赖和提供方目录的准备，并把实际快照路径保存到 `target/phase3-acceptance/m21/prepared.json`。之后可以用 CORDIS_M21_TOOLS、CORDIS_M21_DIRECT_TOOLS、CORDIS_TEST_DEPENDENCIES 与 CORDIS_M21_SCRIPTS_READY 复用准备结果。计时前协调同机任务，结束编译与压力活动；测量记录包含实际运行环境和起止时间。

Linux 使用对应的 `.so` 产物。程序及夹具始终采用当前编译根的 shared 目录，Python 经 Conda base 执行。原始测量由 TestGuard 归档到 `target/test-evidence`；准备阶段保存的快照用于后续计时，完成这一轮测量后可以按其记录路径释放。
