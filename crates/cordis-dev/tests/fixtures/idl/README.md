# WIT 绑定验收

`cargo test -p cordis-dev --test idl -- --nocapture` 从 `plugins/kv-api/kv.wit` 和 `types.wit` 生成 Rust、Python、TypeScript 源码，随后运行真实 SDK 的类型化与通用调用、脚本 Client 与提供方的 CBOR 调用，以及脚本输出的 Rust 回读。

执行环境使用 Cargo、Conda 的 base 环境、Node 与 npm。验收程序通过 `conda run -n base python -m pip install --no-deps --target <验收目录>/python --requirement requirements.txt` 安装 cbor2 5.6.5，通过 `npm ci --ignore-scripts --no-audit --no-fund --prefix <验收目录>/javascript` 按 package-lock.json 安装 TypeScript 5.9.3 与 cbor-x 1.6.0。包管理器缓存位于 `target/idl-dependencies`，各次安装分别位于项目的验收目录。

每次执行在 `target/idl-acceptance` 下创建独立目录，保存生成源码、CBOR 数据与各步骤日志。Rust fixture 通过 `CORDIS_IDL_GENERATED` 读取本次生成源码，依赖编译结果使用 cordis-testkit 计算的 `<编译根>/shared`。

`types.wit` 覆盖 DOC-15 §7 的接口版本、同步与异步方法、record、variant、enum、option、result 和 resource 映射，并涵盖嵌套可选值、borrow、future、stream、64 位整数、固定数组与映射。资源用例检查成功调用、异步取消和上下文停用后的引用回收。KV 用例保留既有方法顺序、字节值、Snapshot 状态映射及 Changed 事件行为。
