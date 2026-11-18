# 跨进程调用与替换测量

本目录的驱动分别测量跨进程 KV 调用（RM-7）、替换停顿（RM-8）和同组原生调用（RM-9）。每个子命令对应一种测量。以下示例从仓库根目录执行，使用共享编译目录。

```sh
build_root="${CARGO_TARGET_DIR:-$PWD/target}"
cargo run --manifest-path benchmarks/multilanguage/driver/Cargo.toml \
  --target-dir "$build_root/shared" -- rpc
```

## 跨进程读取

RM-7 的调用方是独立 cordis-host 分组中的原生 Rust 插件。三个方向分别调用另一分组的 Rust、Python、TypeScript KV 提供方，每向固定 10000 次。准备阶段写入相同的 64 字节值并执行一次正确性读取；每次计时从 Service<Kv>::get 入口前开始，到返回值释放后结束，覆盖 SDK/ABI、Host↔Core↔提供方通信、提供方处理以及回程解码、复制和释放。报告保存全部 sample_ns、中位数、99 分位和总循环时间。

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
