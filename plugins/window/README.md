# 系统窗口插件

`window` 作为独立进程运行。主线程使用 tao 和系统 WebView 显示页面，协议线程经 UP/1 接收核心的生命周期与服务请求。macOS 使用 WKWebView，Linux 使用 WebKitGTK。

部署需要 HTTP 插件提供 `cordis.web@1.0`，并为窗口实例授予管理接口的 `shutdown` 方法：

```toml
[instance.http]
plugin = "http"
component = "http"

[instance.window]
plugin = "window"
component = "window"
grants = { "cordis.manage@1" = ["shutdown"] }
config = { title = "我的应用", width = 1000, height = 720, quit_on_last_window = true }
```

启动时窗口插件从 HTTP 服务取得当前页面地址与会话令牌。窗口关闭策略、标题和尺寸来自实例配置；`quit_on_last_window` 默认开启。Cmd-Q 或关闭最后一个窗口时，插件请求核心有序退出，核心保存检查点并清理全部插件。窗口进程故障后，核心按分组的重启策略恢复它，已有后端实例继续运行。

| 接口 | 行为 |
|---|---|
| `window@1.0.open(url, title, width, height)` | 异步显示窗口，效应及清理登记完成后返回 `u64` 编号 |
| `window@1.0.close(id)` | 异步关闭窗口并释放效应 |
| `window.closed@1.0` | 发布包含窗口编号 `id` 的异步通知 |

Rust 使用方通过 `window-api` 共享接口类型。每个窗口的关闭动作登记在窗口实例的效应中，实例释放时逐项关闭。检查点期间的异步修改等待解冻后继续执行；导出记录已显示窗口的地址、标题和尺寸。默认启动页在 HTTP 服务重建后使用新地址。WebView 采用临时浏览会话，页面业务数据经后端服务持久保存。

`cordis pack --app` 把程序放入发布包的 `Contents/MacOS/cordis-window`，窗口使用应用名称和 Dock 图标。Linux 构建需要 GTK 3 与 WebKitGTK 4.1 开发包。
