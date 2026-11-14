# 网页与全栈插件

## 部署和组件

浏览器运行 cordis-web 及组件业务代码，核心管理唯一的依赖图和组件生命周期。HTTP 原生插件终结 HTTP/WebSocket，并通过 UP/1 转发每个页面的独立连接。部署 HTTP 实例时设置 `forwarder = true`；[网页部署配置](../templates/web-deployment.toml) 可并入项目现有 cordis.toml。

| 创建命令 | 清单与入口 |
|---|---|
| `cordis new-plugin panel --web` | socket=web，组件 panel |
| `cordis new-plugin panel --rust --web` | native 后端 panel，网页组件 panel/web |
| `cordis new-plugin panel --python --web` | Python 后端 panel，网页组件 panel/web |
| `cordis new-plugin panel --ts --web` | Node 后端 panel，网页组件 panel/web |

同一插件的后端与网页部分共享清单版本。web.assets 是相对插件目录的资源路径，web.entry 相对 assets 解析，web.mount 声明挂载路径。原生全栈模板的 build.sh 编译后端并把动态库复制到插件目录；Python 与 TypeScript 部分沿各自章节完成依赖和构建。

脚手架的 web/index.js 是可直接加载的 ES 模块，从 `/_cordis/cordis-web.js` 导入 defineComponent、Frozen 等浏览器 API。每个网页模块默认导出一个 `defineComponent({...})` 对象，宿主按入口清单的 component 确定该对象的实际组件名。HTTP 默认页面加载当前入口清单，业务组件把 DOM 放入 `main#cordis-components`。使用自己的构建工具时，将宿主导入地址保留为 external，使所有组件共用同一页面宿主。

## 页面连接

默认 HTTP 页面已经调用 connect。自定义页面加载 `/_cordis/session.js`，再执行 `connect({token: globalThis.CORDIS_SESSION.token})`；每个页面建立一条宿主连接。connect 返回 `{close, completed}`，close 请求结束连接与重连循环，completed 等待整个连接流程结束。

正式应用应提供自己的页面外壳。HTTP 的 `assets` 指向外壳目录，`index` 选择其中的入口文件；该入口保留以下协议连接：

```html
<!doctype html>
<html lang="zh-CN">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>应用工作台</title>
<script src="/_cordis/session.js"></script>
<body>
  <header>应用工作台</header>
  <main id="cordis-components"></main>
  <script type="module">
    import {connect} from '/_cordis/cordis-web.js';
    connect({token: globalThis.CORDIS_SESSION.token});
  </script>
</body>
</html>
```

协议路径、会话对象与挂载标识属于宿主接口，用户可见文本由应用定义。默认开发外壳的标题来自 HTTP 的 `title`，缺省为空；连接状态在中断或结束时显示中性提示。组件的替换清理限定在它登记的 DOM 与效应，外壳由应用保留。

可选字段 endpoint、manifest 分别默认为 `/_cordis/socket` 和 `/_cordis/manifest.json`；reconnectAttempts 表示初次连接后的重连次数上限，默认为20次，reconnectDelayMs 默认为500毫秒。status 回调接收 connecting、active、disconnected、closed；active 对应 WebSocket 已打开，组件是否已完成 apply 由核心实例状态和实际界面确认。页面 pagehide 触发关闭，重连前从 `/_cordis/session.json` 取得当前令牌。

## 服务与页面域

网页组件通过生成的 WIT Client 注入后端服务，Context.get、provide、effect、exportState 的调用形态见[TypeScript 插件](typescript.md)。DOM、事件监听器、在途界面操作和服务客户端由同一效应持有；清理先撤下控件并停止接收事件，再等待已有操作并 `close(client)`。Effect.close 归还外部控制句柄，效应继续由核心持有；Effect.dispose 请求并等待实际清理。派生 Context 通过自身的 close() 归还视图份额。

网页任务通过 `context.task(signal => ...)` 接收 AbortSignal。Node 入口的 withSignal、currentSignal 属于进程 SDK 的调用上下文接口。网页连接结束时，宿主执行仍持有的本地 defer，并合并同一清理回调的在途调用；完成清理后继续重连。远程调用错误通过 CordisError.code 交给调用方处理。

每个页面拥有自己的服务域。核心通过 PageNamespace 标记页面视图，并按 `(页面连接编号, 域名)` 识别显式命名域；同页同名域共享身份，不同页保有各自服务。`await context.isolate(interface, "tenant-a")` 进入当前页面中的命名域，省略 realm 时创建新的独有域。后端普通服务按组件声明注入，跨页面共享业务数据由后端服务管理。

`await context.plugin(component, config)` 在当前视图下装入当前插件描述符中的组件。网页模块的描述符包含默认导出的这一个组件，因此自身子组件使用其实际组件名，并通过配置区分父级与子级行为。纯网页名为 panel 的组件使用 panel，全栈后端组件名为 panel 时使用 panel/web。

服务依赖在提供方 Active 后可用。子组件依赖父组件当前加载代提供的新服务时，在父组件进入 Active 后调用 context.plugin 挂载使用方，例如由后续用户事件触发。

必需依赖退出时，页面组件转为 Pending 并执行界面清理；依赖恢复后，核心重新激活组件。关闭页面时，核心释放该页面的子实例、服务和监听器；同一应用的其他页面继续运行。HTTP 重启后，cordis-web 读取新会话令牌并重新连接。

## 状态与替换

组件声明状态模式、版本范围并登记 exportState。浏览器 State 以 Map 表示，写入操作调用 frozen.check。只在当前页面保持的输入、选择和计数进入页面快照；需要跨刷新保留的业务数据写入后端服务。

纯网页替换依次导出状态、清理旧效应、加载带新序号 URL 的模块并继承状态。全栈替换先使后端 Active，再切换各页面；任一步失败时恢复同一事务的后端、页面版本和状态。多个插件的变化使用 replace_batch，让 watch 窗口保持整体提交与整体恢复。

运行验证观察真实页面控件和后端数据，记录各页独立状态与页面文档是否保持。调用管理接口 configure 时，读取配置的组件在新激活中采用该值；remove 回收对应资源登记和子实例。
