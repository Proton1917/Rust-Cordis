# 同宿主原生调用测量

此基准测量已取得服务句柄后的一次 `Counter::value` 调用。M9 修订 `dc93da1` 的消费方和当前 SDK 的消费方使用相同源码，调用同一个提供方。它们分别以 Rust 1.85 release 构建，宿主与测量器使用仓库固定工具链。样本衡量 ABI v1 的类型化调用，跨进程往返和整项业务的延迟有各自的测量入口。

[settings.json](settings.json) 给出轮数、预热、每轮调用次数和比值判据。一次专项测量可从仓库根目录发起：

```sh
build_root="${CARGO_TARGET_DIR:-$PWD/target}"
cargo run --manifest-path benchmarks/multilanguage/driver/Cargo.toml \
  --target-dir "$build_root/shared" -- direct
```

驱动可通过 `CORDIS_M21_TOOLS` 和 `CORDIS_M21_DIRECT_TOOLS` 使用已有的工具快照。解释结果时应同时考虑当时的机器负载和各轮原始样本。[M14 数据](../../docs/measurements/m14-direct-call.json) · [M21 两个测量窗口](../../docs/measurements/m21-performance/README.md)
