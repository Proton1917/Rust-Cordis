# Python SDK 真实进程验收

验收入口为 run.py。Rust 验收调用方使用 `cordis_testkit::build::shared_target_dir` 选择共用 Cargo 目录，构建核心、原生宿主、开发工具、kv-mem、counter、kv-contract 与 `crates/cordis-runtime/fixtures/plugin`，再通过 `snapshot_artifacts` 保存本次程序和库。把该快照目录通过 `--tools` 传给 Python 入口。

工具目录包含 `cordis`、`cordis-dev`、`cordis-host`、`cordis-runtime-fixture`、`libkv_mem.dylib`、`libcounter.dylib`、`libkv_contract.dylib`。以下命令在 macOS 验收环境运行：

```bash
conda run -n base python tests/python-sdk/run.py \
  --tools target/python-sdk-tools \
  --evidence target/python-sdk-acceptance
```

`--evidence` 指定本次新建证据目录。入口复制工具副本、按 SDK requirements.txt 安装 cbor2 到该目录，再调用真实 cordis-dev 生成资源绑定。已有匹配依赖目录可通过 `--dependencies` 指定。`--source` 可选择待验收 SDK 和插件源码根目录，缺省为本仓库。

| 场景 | 验收事实 |
|---|---|
| smoke | 原生与 Python 双向调用；scoped 元数据；异步效应等待、清理与退出 |
| contract | 原生 kv-contract 的字节、覆盖、排序、flush 和状态继承断言 |
| resources | 资源构造、借用、镜像独立持有、取消、超帧错误、私有域子组件 |
| transfer | 中转资源方法接收和返回资源；事件资源在回调结束后继续调用 |
| realms | tenant-a 与 tenant-b 的同名键隔离 |
| migration | 原生 → Python → 失败回退 → 原生，及 process → native 的 replace_batch |

`--scenario` 可重复指定受当前修改影响的场景。每个场景在独立目录保存部署、核心输出和实际结果。Python 进程清单使用 `conda run --no-capture-output -n base python`，UP/1 标准流直接连接核心。源码、状态、日志和构建工具副本位于指定工作区。
