# 内核事件与服务读取测量

`crates/cordis/tests/event_hot_paths.rs` 提供三项分配预算测试和一个默认忽略的计时入口。分配窗口仅覆盖同步派发或服务读取，准备上下文与注册监听器在窗口之外完成。服务读取使用已有绑定，空瀑布终点捕获一个整数，以覆盖实际需要闭包装箱的基线路径。

在仓库根目录运行分配检查：

```sh
cargo test --locked -p cordis --test event_hot_paths
```

显式运行 release 计时，每个场景默认预热一万次，再独立计数一千次调用的分配，随后执行九轮、每轮十万次调用：

```sh
cargo test --release --locked -p cordis --test event_hot_paths \
  measure_event_hot_paths -- --ignored --nocapture --test-threads=1
```

可通过 `CORDIS_BENCH_ITERATIONS` 和 `CORDIS_BENCH_ROUNDS` 设置正整数迭代次数和轮数。输出行的字段依次为：

```text
measurement,scenario,round,iterations,elapsed_ns,allocation_iterations,allocations
```

`elapsed_ns / iterations` 是本轮批量平均调用成本；`allocations / allocation_iterations` 是独立计数窗口中的每次堆分配次数。计时窗口关闭计数。观察器场景同时有一个同步监听器；服务中间件场景有一个原样调用 `next.run()` 的 `InternalGet` 监听器。

比较两个修订时，把同一份测试文件放入两个隔离工作目录，各自设置独立的 `CARGO_TARGET_DIR`，分别构建 release 测试可执行文件。构建完成后交替运行基线和候选，保证 CPU 亲和性、迭代次数和机器负载条件一致；保留全部样本与源码身份。基线预算检查预期能检出旧分配，性能入口可用 `--exact measure_event_hot_paths --ignored --nocapture --test-threads=1` 单独运行。

[内核事件与服务读取测量记录](../../docs/measurements/kernel-event-hot-paths.md) 包含 v0.3.1 基线、候选文件摘要、配对方法和结果。这些场景衡量进程内内核调用，整项部署性能需结合业务和跨进程测量解释。
