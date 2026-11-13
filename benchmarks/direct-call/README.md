# 同组直接调用基准

RM-9 比较 M9 提交 `dc93da1` 的类型化调用与当前 SDK。两个 consumer 使用同一份源码，provider 和两个 consumer 均以 Rust 1.85 release 编译，Host 与测量器使用固定核心工具链的 release。测量前已取得服务句柄；计时覆盖每次完整的 `Counter::value` 调用。

固定执行 9 轮，每轮预热 10000 次，再计时 100000 次；两种实现逐轮交替顺序。预定判据为当前/基准的中位数比值至多 1.05、最大值比值至多 1.15。完整样本保存至结果文件，测量到固定轮次即结束。

在仓库根目录使用共用测量驱动。历史源码由 Git 取得，程序、两个 consumer 和 provider 统一进入当前编译根的 shared 目录：

```bash
build_root="${CARGO_TARGET_DIR:-$PWD/target}"
cargo run --manifest-path benchmarks/multilanguage/driver/Cargo.toml \
  --target-dir "$build_root/shared" -- direct
```

测量前结束本次其他编译和压力任务，并与同机协作任务协调运行窗口。驱动使用平台动态库扩展名，生成独立结果目录，成功后由 TestGuard 归档原始样本与日志。已准备的宿主和对照工具可分别通过 CORDIS_M21_TOOLS、CORDIS_M21_DIRECT_TOOLS 指定，计时阶段复用这些快照。

M14 的样本在 `docs/measurements/m14-direct-call.json`。M21 保留负载重叠的首轮与停载重测的全部数据；两次都观察到前几轮时延较高，完整样本及既定比值共同解释该次结果。预热长度和交替采样方式属于后续独立研究，当前入口保持原轮次和判据。
