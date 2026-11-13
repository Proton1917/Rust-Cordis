# ADR-0037：独立窗口进程与系统 WebView

| 项 | 内容 |
|---|---|
| 状态 | 采纳 |
| 依据 | DOC-15 §11.6、AU-17、Q0、CP-7、CP-10 |

## 问题

窗口与 WebView 的平台事件循环需要进程主线程。窗口进程同时执行 UP/1 生命周期、对象调用和资源回退，并把退出动作交给核心的管理服务。

## 选项与判据

直接调用 AppKit 与 WebKitGTK 会引入平台指针与回调的维护；使用通用窗口与系统 WebView 库可以复用平台适配。选型关注 macOS 的 WKWebView、Linux 的 WebKitGTK、主线程事件循环、跨线程命令投递和菜单事件。

## 决定

1. `plugins/window` 是独立可执行程序，使用 tao 0.37.2、wry 0.57.2 与 muda 0.21.2。tao 管理窗口和主线程事件循环，wry 装入系统 WebView，muda 提供应用菜单和 Cmd-Q。插件源码采用安全 Rust；平台细节由库管理。Linux 构建依赖 GTK 3 与 WebKitGTK 4.1 开发包。
2. UP/1 运行在独立 Tokio 线程中，复用 cordis-up 的帧、引用和截止时间实现。主线程接收窗口命令，通过一次性回复通道交回实际结果；服务调用完成以窗口操作结果为准。标准输入结束或核心请求 shutdown 时停止界面循环。
3. `window@1.0` 提供异步 `open(url, title, width, height) -> u64`、`close(id)`，以及 `window.closed@1.0` 事件 `{ id }`。窗口及其关闭动作按 PR-1 登记在实例的效应内。实例清理后释放全部窗口，用户主动关闭后释放对应的效应句柄。
4. 窗口组件声明 `cordis.web@1.0` 和 `cordis.manage@1.0` 依赖。部署为其授予 `shutdown`。启动页面由 Web 服务取得；Cmd-Q 和配置启用的最后窗口关闭请求管理服务有序退出，随后按核心清理流程退出窗口进程。
5. `cordis-window` 在 macOS 发布包中放入 `Contents/MacOS/`，与核心共享 Bundle 名称。核心保持后台属性，窗口进程在创建实际窗口时设置 Regular activation policy。描述符探测阶段采用 Accessory policy。一个发布应用配置一个窗口插件进程，该进程可管理多个窗口。
6. 窗口配置和打开的页面地址作为可导出状态保存。核心按既有 CP-6 重启窗口进程，重新 apply 并恢复窗口；后端依赖和纪元仍由唯一内核处理。
7. WebView 使用临时浏览会话。页面组件状态通过核心的导出与继承保存，跨页面与长期业务数据经后端服务存储；默认启动页在 Web 提供方重建后取得当前地址与启动令牌。
8. `open`、`close` 是异步操作。检查点冻结期间按上下文保存等待中的调用，解冻后依次执行；请求数量受 UP/1 在途额度约束。用户关闭请求按窗口编号合并。导出只记录已显示窗口，显示动作同样等待解冻。上下文清理回收等待者与窗口。

## 后果

GUI 库及其平台依赖进入窗口插件。打包工具负责 Bundle 布局与进程命令改写。AU-17 在真实 `.app` 中检验窗口进程恢复、后端纪元、Cmd-Q 清理与 Dock 图标，并保存实际窗口截图。HTTP 静态服务的阶段前移见 ADR-0038。

## 资料

- [tao 0.37.2](https://docs.rs/tao/0.37.2/tao/)：事件循环、窗口和 macOS activation policy。
- [wry 0.57.2](https://docs.rs/wry/0.57.2/wry/)：WKWebView、WebKitGTK 和主线程窗口集成。
- [muda 0.21.2](https://docs.rs/muda/0.21.2/muda/)：原生菜单、快捷键与事件投递。
