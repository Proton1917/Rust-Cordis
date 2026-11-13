# 网页插件示例

web-notes 是页面笔记组件，保存当前页面的文本和本地计数。kv-dashboard-rust 与 kv-dashboard-python 分别附带 Rust、Python KV 提供方，页面通过生成的 kv@1.0 Client 执行字节读写、列出存储键，并保存各页面独立的输入框和计数状态。

三个页面共享 source/web-ui 中的控件和样式源码，构建后各插件的 web/index.js 独立发布。页面从 `/_cordis/cordis-web.js` 导入宿主 API。组件创建的 DOM、事件监听、在途调用和服务引用都由其效应清理。

在此目录安装构建依赖并生成网页资源：

```bash
npm ci --ignore-scripts
npm run build
```

准备后端时使用当前源码对应的 kv-mem 动态库，并传入 Rust-cordis 的工作区路径：

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

主要界面控件采用可访问名称：笔记内容、存储键、存储内容、写入存储、读取内容、刷新键列表、计数加一和本地计数。组件根节点的 data-plugin 分别为 web-notes、kv-dashboard-rust、kv-dashboard-python，供多页面运行检查定位。

设计变量与按钮尺寸参考 Open Claude UI 02d021fceaf2a907f9e63a8125621bdda1b85d60，MIT 许可证保存于 source/web-ui/LICENSE.open-claude-ui。组件样式限定在 ShadowRoot，支持窄屏、深色偏好与减少动态效果。
