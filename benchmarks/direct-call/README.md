# 同组直接调用基准

RM-9 比较 M9 提交 `dc93da1` 的类型化调用与当前 SDK。两个 consumer 使用同一份源码，provider 和两个 consumer 均以 Rust 1.85 release 编译，Host 与测量器使用固定核心工具链的 release。测量前已取得服务句柄；计时覆盖每次完整的 `Counter::value` 调用。

固定执行 9 轮，每轮预热 10000 次，再计时 100000 次；两种实现逐轮交替顺序。预定判据为当前/基准的中位数比值至多 1.05、最大值比值至多 1.15。完整样本保存至结果文件，测量到固定轮次即结束。

在仓库根目录准备历史源码和产物：

```bash
mkdir -p target/direct-call-baseline
git archive dc93da1 | tar -x -C target/direct-call-baseline
cargo +1.85.0 build --release --manifest-path benchmarks/direct-call/baseline/Cargo.toml --target-dir target/direct-call/baseline
cargo +1.85.0 build --release --manifest-path benchmarks/direct-call/current/Cargo.toml --target-dir target/direct-call/current
cargo +1.85.0 build --release --manifest-path crates/cordis-host/tests/fixtures/Cargo.toml -p loader-fixture --target-dir target/direct-call/provider
cargo build --release -p cordis-host --target-dir target/direct-call/host
cargo build --release --manifest-path benchmarks/direct-call/driver/Cargo.toml --target-dir target/direct-call/driver
```

确认其他编译与压力任务已结束后执行：

```bash
target/direct-call/driver/release/cordis-direct-call-benchmark \
  --host target/direct-call/host/release/cordis-host \
  --provider target/direct-call/provider/release/libloader_fixture.dylib \
  --settings benchmarks/direct-call/settings.json \
  --output target/direct-call/result
```

Linux 将设置文件和命令中的动态库后缀改为 `.so`。输出目录由本次测量创建，保留既有结果。M14 的已执行样本在 `docs/measurements/m14-direct-call.json`；其中各轮耗时呈下降趋势，报告预定判据的结果和全部样本，时延差异按该次测量条件解释。
