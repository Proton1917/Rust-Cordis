---
name: cordis-plugin
description: 编写、验证和部署 Cordis Rust 原生插件、共享接口与接口契约套件，处理效应清理、服务注入、状态继承和运行中的插件替换。
---

# Cordis 插件

使用 `cordis-sdk` 实现插件，交付可独立编译的动态库及对应部署配置。核心通过 UP/1 管理生命周期，原生代码在 `cordis-host` 中执行；同组原生服务可以直接走 ABI 类型化方法表。

先检查项目的 Cargo 依赖和 `cordis.toml`，沿用既有接口及 SDK 修订。`cordis version` 显示核心版本，`cordis-dev version` 显示开发工具支持范围。原生插件和接口库支持 Rust 1.85；核心和开发工具使用仓库固定的工具链。

## 工作入口

- 新项目使用 `cordis new demo`，在项目中使用 `cordis new-plugin hello --rust`。
- 插件名采用小写字母开头的小写字母、数字和短横线；生成的逻辑插件名、组件名与部署引用共用该名称。
- 构建插件使用 `cargo build --target-dir target`，产物与生成配置的 `target/debug` 对应，也适用于设置了全局 `CARGO_TARGET_DIR` 的环境。
- 新服务把接口 trait 和数据类型放进独立接口 crate；提供方与使用方共同依赖它。先读 [Rust 接口](references/rust-api.md)。
- 有任务、线程、文件、监听器或可变状态时，读 [资源与状态](references/resources-state.md)，把运行中的行为和清理一起实现。
- 编写契约或部署替换时，读 [验证与部署](references/verification-deployment.md)。

## 插件的组成

实现 `Component`，声明 `NAME`、可反序列化的 `Config`，并在 `inject`、`provides`、`listens` 中列出使用的接口和事件。`resolve` 校验配置，`apply` 执行本次激活；原生库通过 `export_plugin!` 导出组件。库的 Cargo `crate-type` 包含 `cdylib`，panic 策略保持 `unwind`。

接口用 `#[interface(name = "名称", version = "主版本.次版本")]` 声明，接口实现再加 `#[interface]`。提供服务使用 `ctx.provide::<dyn I>(Arc::new(实现))`，取得服务使用 `ctx.get::<dyn I>()`。访问前声明注入关系；必需服务缺席时，核心让实例保持 `Pending`，服务可用后继续加载。

替换需要保持逻辑插件名、组件名以及所提供接口的主版本；兼容演进在末尾追加方法并提升次版本。配置通过实例条目传入，宿主分组由 `group` 指定，上下文配置分组使用 `scope`。

## 运行与交付

启动 `cordis run` 后，以 `cordis status` 检查实例、库、错误和替换报告。使用真实消费者调用服务，确认行为、清理和状态继承；需要契约验证时运行 `cordis verify`。替换前后比较实际业务状态，并确认依赖方已重建。

`watch` 的构建变化通过管理接口提交为完整批次；管理授权写在基础 `cordis.toml` 的 `grants`。新版本失败时恢复原版本、原配置和原状态。持久数据保存在 `ctx.data_dir()` 返回的目录中，访问该目录前声明 `cordis.data-dir@1` 依赖。

交付源码、构建与运行命令、使用的 SDK 修订和实际行为验证结果。核心程序与插件分别交付，插件仓库保存自己的源码和配置。
