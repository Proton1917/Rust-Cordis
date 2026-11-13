# ADR-0042 原生插件组件库与动态入口

| 项 | 内容 |
|---|---|
| 状态 | 实施依据 |
| 影响 | DOC-13 crate 职责；kv-mem、counter、SDK 测试入口、kv-evolution、发布测试与共享编译目录 |

## 问题

共享 Cargo target 承载工作区和独立夹具的编译图。kv-mem 与 counter 同时声明 cdylib、rlib，Cargo 将两类输出写为固定名称，包括 debug/deps/libkv_mem.rlib 与 libcounter.rlib。不同编译图的依赖特性与 crate 元数据可以变化，而两个 rlib 路径保持相同。

M19 第一次完整验收中，kv-evolution 的 KvStore::apply、provides、state 调用报告 cordis_sdk trait 身份冲突。Cargo 依赖树只有一个 SDK 版本；两个 kv-mem 编译单元记录具有不同特性及依赖元数据，却引用同一 rlib 文件。该故障发生在共享目录的产物身份上。原始证据位于 target/cleanup-verification-m19/round-1.log。

## 决定

| 对象 | 职责与产物 |
|---|---|
| plugins/kv-mem-component | KvStore 与内存 KV 的完整实现；普通 Rust 库 |
| plugins/counter-component | CounterConfig、Counter 与计数生命周期；普通 Rust 库 |
| plugins/kv-mem | 引用 KvStore 并导出 kv 插件入口；纯 cdylib，动态库名保持 kv_mem |
| plugins/counter | 引用 Counter 并导出 counter 插件入口；纯 cdylib，动态库名保持 counter |
| kv-evolution | 直接依赖 kv-mem-component，复用同一份 KvStore 实现 |
| cordis-sdk 的 loopback 测试 | 直接依赖组件库，在测试模块内通过 export_plugin 构造各自真实入口 |

普通 Rust 组件库的依赖产物由 Cargo 使用带编译单元后缀的名称保存。动态插件入口继续使用稳定的文件名，持锁构建并复制给实际运行场景。共享目录规则保持原值，各编译图可以同时保有各自的 Rust 类型身份。

export_plugin 在 cfg(test) 下保留 Rust 符号名，使一个测试程序可以包含多个入口模块；生产构建继续导出 CORDIS_PLUGIN_ABI 和 cordis_plugin_v1。函数签名、HostV1、PluginV1、服务方法表、状态模式及组件行为保持 ABI v1。

kv-file、kv-contract、watch、http 的调用来源为动态宿主及其模块内测试，因此构建类型统一为 cdylib。组件复用采用独立 Rust 库，脚手架和作者模板生成纯动态入口。宏接受 name、version 与 components；测试符号由 cfg(test) 选择。

## 依赖与发布

组件库只依赖 SDK、接口 crate 以及实际实现使用的 serde。组件源代码各保留一份，动态入口和测试入口共同引用这些类型。SDK 对组件库的依赖属于 dev-dependencies。

`cargo build -p kv-mem` 和 `-p counter` 继续产出原名动态库。发布测试生成的 memory/counter 工程引用组件库，并编译自己的入口文件，确保打包行为读取当前源码对应的实际产物。生成项目和发布目录仍沿既有运行时配置装配。

## 验证

复用已有失败记录，修复后执行实际 kv-evolution 构建及原 runtime migration 场景，覆盖版本切换、状态迁移、失败回退和 Rust 1.85 使用方。SDK loopback 继续经过真实 ABI 入口执行内存 KV 与计数组件。完整工作区验收重新开始固定连续两轮，统计同一范围的测试现场和证据目录。
