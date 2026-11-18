# 网页插件示例

打开两个页面就能观察服务和页面的不同寿命。`web-notes` 的文字与计数归各页面所有；两个 KV 面板通过共享存储读写同一份字节值，输入框和计数仍由各页独立维护。KV 提供方分别采用 Rust 与 Python。

三个页面共享 source/web-ui 中的控件和样式源码，构建后各插件的 web/index.js 独立发布。页面从 `/_cordis/cordis-web.js` 导入宿主 API。组件创建的 DOM、事件监听、在途调用和服务引用都由其效应清理。

## 构建网页资源

在 `examples/web/` 执行：

```bash
npm ci --ignore-scripts
npm run build
```

## 准备后端并启动

从本目录将已构建的 HTTP 和 KV 动态库传给准备脚本：

```bash
node prepare-backends.mjs \
  --workspace /path/to/Rust-cordis \
  --native /path/to/build-root/shared/debug/libkv_mem.dylib \
  --http /path/to/build-root/shared/debug/libcordis_http.dylib
```

原生库的来源为工作区 `cargo build -p cordis-runtime -p cordis-host -p cordis-http -p kv-mem --target-dir <编译根>/shared`，Linux 使用 `.so` 扩展名。准备脚本复制该产物，并把正式 kv-py 和 Python SDK 模块放入 Python 示例。Python 运行环境安装 SDK 声明的 cbor2 依赖；发布包的解释器和依赖由 cordis pack 的 Python 运行时装配负责。

把上述构建目录的 debug 路径加入 PATH，使 cordis 与 cordis-host 来自同一次构建，然后从示例目录选择一个配置运行：

| 命令 | 页面 | 实例行为 |
|---|---|---|
| `cordis run --config cordis-notes.toml` | `http://127.0.0.1:8070/` | 页面笔记与本地计数 |
| `cordis run --config cordis-rust.toml` | `http://127.0.0.1:8071/` | Rust 内存 KV 与网页组件 |
| `conda run -n base cordis run --config cordis-python.toml` | `http://127.0.0.1:8072/` | Python KV 与网页组件 |

三个配置分别保存 state/notes、state/rust、state/python。浏览器可同时打开两个页面，通过共享存储键交换内容，各页面的本地计数独立变化。使用 Ctrl+C 有序关闭本次运行。

两种 KV 实现的逻辑名均为 kv，组件为 kv-store，浏览器组件为 kv-store/web。分别作为全栈部署来源，也可用于同一逻辑插件的 Rust/Python 替换。前端状态模式统一为 web.kv-dashboard@1，后端状态模式为 kv.snapshot@1。web-notes 使用独立的 web.notes@1。

## 资源与许可

页面控件位于 [source/web-ui](source/web-ui)，样式封装在各自的 ShadowRoot 中。[样式参考的许可证](source/web-ui/LICENSE.open-claude-ui)随源码提供。网页模块的连接、清理与状态迁移接口见[浏览器宿主](../../sdks/typescript/cordis-web/README.md)。
