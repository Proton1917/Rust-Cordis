# Cordis 网页宿主

`cordis-web` 在浏览器中执行网页组件。每个页面通过 WebSocket 连接 `http` 插件，后者将 UP/1 消息转发到核心。核心管理页面子实例、服务依赖、效应与版本替换；浏览器执行组件的 `apply`、界面操作、清理回调和状态导出。

网页插件使用 ES 模块默认导出一个组件定义。每个页面拥有独立的连接和组件实例，网页组件通过生成的 Client 调用 Rust、Python 或 TypeScript 提供方。

## 构建与资源入口

构建使用 Node 22 及以上版本。在仓库根目录执行：

```sh
cd sdks/typescript
npm ci --ignore-scripts --no-audit --no-fund
npm run build
```

统一构建入口检查 TypeScript 配置与类型，生成 `cordis-sdk/dist`，并将网页 bundle 写入以下位置：

- `sdks/typescript/cordis-web/dist/cordis-web.js`
- `plugins/http/assets/cordis-web.js`

HTTP 内置资源随 `cordis-http` 编译进入动态库，SDK 构建之后在仓库根目录执行 `cargo build -p cordis-http`。网页插件从同源路径 `/_cordis/cordis-web.js` 导入作者接口。构建依赖固定在 workspace 的 `package-lock.json`；脚本通过 Node API 调用 TypeScript 和 esbuild，适配 `.npmrc` 中的 `bin-links=false`。

纯网页插件的 `cordis-plugin.toml` 示例：

```toml
name = "page-counter"
version = "0.1.0"
socket = "web"

[web]
assets = "web"
mount = "/page-counter"
entry = "index.js"
```

`assets` 相对插件目录，`entry` 相对 `assets`，因此该入口对应 `web/index.js`。`mount` 定义资源挂载路径，组件在 `apply` 中把 DOM 加入页面容器。全栈插件在原生或进程插件清单中增加同样的 `[web]` 段，页面子实例挂在活动后端组件之下。

## 页面连接

HTTP 默认页面已经加载 `/_cordis/session.js` 并调用 `connect`。自定义页面可采用同样的入口：

```html
<script src="/_cordis/session.js"></script>
<main id="cordis-components"></main>
<script type="module">
  import {connect} from "/_cordis/cordis-web.js";

  const connection = connect({
    token: globalThis.CORDIS_SESSION.token,
    status(state, error) {
      document.documentElement.dataset.connection = state;
      if (error) console.error(error);
    },
  });
</script>
```

`connect(options)` 返回 `{close(): void, completed: Promise<void>}`。`close()` 请求结束当前连接及重连循环，`completed` 在连接流程结束后完成。页面的 `pagehide` 事件调用同一关闭路径。

| 选项 | 类型 | 缺省值 | 含义 |
|---|---|---|---|
| `token` | `string` | 调用方提供 | 当前 HTTP 激活的会话令牌 |
| `endpoint` | `string` | `/_cordis/socket` | WebSocket 端点，协议随页面的 HTTP/HTTPS 转为 WS/WSS |
| `manifest` | `string` | `/_cordis/manifest.json` | 当前网页模块清单的地址 |
| `reconnectAttempts` | `number` | `20` | 初次连接之后的重连次数上限 |
| `reconnectDelayMs` | `number` | `500` | 相邻连接尝试之间的等待，单位毫秒 |
| `status` | `(state, error?) => void` | 调用方可提供 | 连接状态及本次错误通知 |

`status` 的状态值为 `connecting`、`active`、`disconnected`、`closed`。其中 `active` 表示 WebSocket 已打开；各组件的生命周期状态由核心实例目录记录。清单请求通过 `Authorization: Bearer <token>` 认证，WebSocket 握手使用 `cordis-up1` 与令牌子协议，HTTP 插件同时检查 Origin。

重连前，宿主从 `/_cordis/session.json` 读取当前令牌，再取得清单并建立新连接。HTTP 默认监听地址在实例数据目录中保存，进程恢复后沿用该地址并轮换令牌。旧连接的本地清理完成后，宿主进入下一次连接尝试。

## 定义组件

```javascript
import {defineComponent, Frozen} from "/_cordis/cordis-web.js";

export default defineComponent({
    name: "page-counter",
    stateSchema: "page-counter.page",
    stateVersionMin: 1,
    stateVersionMax: 1,
    async apply(context) {
        let count = context.inherited?.get("data").get("count") ?? 0;
        const frozen = new Frozen();
        await context.exportState("page-counter.page", 1, () => ({count}), frozen);

        const effect = await context.effect(async scope => {
            const panel = document.createElement("section");
            const output = document.createElement("output");
            const button = document.createElement("button");
            const events = new AbortController();
            output.value = String(count);
            button.textContent = "计数加一";
            panel.append(output, button);
            await scope.defer(() => { events.abort(); panel.remove(); });
            button.addEventListener("click", () => {
                if (frozen.frozen) return;
                frozen.check();
                output.value = String(++count);
            }, {signal: events.signal});
            document.querySelector("main#cordis-components").append(panel);
        }, "counter-view");
        effect.close();
    },
});
```

`defineComponent(component)` 保留并返回组件对象，默认导出由宿主加载。`name` 与 `apply(context, config)` 是组件的必填成员；可选成员如下：

| 成员 | 类型与作用 |
|---|---|
| `inject` | `Record<string, boolean>`；接口标识映射到依赖的必需性，`true` 表示必需依赖 |
| `provides` | `string[]`；组件提供的接口标识 |
| `listens` | `string[]`；组件订阅的事件标识 |
| `stateSchema` | `string`；接受的继承状态模式 |
| `stateVersionMin`、`stateVersionMax` | `number`；接受的状态版本闭区间 |
| `resolve(config)` | 返回 `void` 或 Promise；在配置提交前检查业务规则 |

宿主根据核心发布的 `Entry.component` 确定实际组件名。纯网页插件使用其网页组件名，全栈页面使用 `<后端组件名>/web`。`Entry` 还包含插件 `name`、`version`、入口 `url` 和资源 `mount`。

## Context 与服务调用

`apply` 接收当前加载代的 `Context`。`context.number` 是 bigint 上下文编号，`context.config` 是配置值，`context.inherited` 保存继承状态。CBOR 映射在继承值中表现为 `Map`，上例使用 `.get("data")` 读取状态数据。

| 接口 | 作用 |
|---|---|
| `get(interfaceName, binding, Client)` | 返回生成的 Client，调用通过当前上下文的依赖视图路由 |
| `provide(interfaceName, provider, binding, scoped = false)` | 登记服务对象；作用域服务以 `scoped = true` 启用提供方的元数据视图 |
| `service(interfaceName, provider, binding)` | 导出可在参数、结果或事件中传递的资源标签 |
| `effect(body, label = "typescript-effect", asynchronous = false)` | 创建受核心生命周期管理的效应，body 接收作用域 Context，返回 `Effect` |
| `defer(cleanup, asynchronous = true)` | 在效应回调的作用域中登记清理 |
| `on(event, callback, asynchronous = false, binding?)` | 登记事件监听；callback 接收 Context 与事件载荷 |
| `emit(event, value = null, asynchronous = false)` | 发送事件值及其中的资源引用 |
| `isolate(interfaceName, realm = null)` | 派生接口隔离视图；省略域名时建立新域，指定域名时使用当前页面的同名域 |
| `intercept(interfaceName, metadata)` | 派生包含接口元数据的视图 |
| `plugin(component, config = null)` | 在当前视图挂载本插件描述符中的子组件，返回 bigint 实例编号 |
| `task(function_, label = "typescript-task")` | 执行接收 AbortSignal 的异步任务，效应清理时发送取消并等待任务结束 |
| `exportState(schema, version, exporter, frozen = new Frozen(), thaw?)` | 登记状态导出、冻结与可选的恢复回调，返回 Frozen |
| `configuration()`、`inheritedState()` | 查询当前配置与继承状态 |
| `isCurrent()` | 查询当前加载代是否仍然有效 |
| `log(message, level = 2)` | 通过核心记录组件日志 |
| `close()` | 归还派生 Context 所持的远程视图引用 |

除 `service` 和 `close` 外，上表方法均返回 Promise；`isolate`、`intercept` 的结果为派生 Context。服务与事件绑定由 WIT 生成：

```sh
cordis-dev idl gen --lang ts --output web/kv.ts interfaces/kv.wit
```

插件导入生成模块，并使用其 Client：

```javascript
import {close} from "/_cordis/cordis-web.js";
import * as binding from "./kv.js";

const effect = await context.effect(async scope => {
    const store = await scope.get("kv@1.0", binding, binding.KvClient);
    await scope.defer(() => close(store));
    await store.put("message", new TextEncoder().encode("Cordis"));
    const bytes = await store.get("message");
    await scope.log(new TextDecoder().decode(bytes));
}, "store-operation");
effect.close();
```

该片段放在声明 `inject: {"kv@1.0": true}` 的组件 `apply` 内，生成的 TypeScript 绑定与插件入口一起编译为浏览器模块。`close(client)` 归还客户端持有的服务引用；派生视图通过该视图的 `close()` 归还。`BindingTransport`、`Remote` 与 `CordisError` 也由网页入口导出，供绑定与资源调用使用。

## 效应、命名域与状态

DOM、监听器、定时器和服务 Client 在同一效应中取得，并通过 `scope.defer` 按所需顺序释放。具有在途异步操作的界面在清理时先撤下 DOM、取消事件来源，再等待操作结束并释放 Client。`Effect.close()` 归还外部控制句柄；`Effect.dispose()` 发起并等待实际清理；`Effect.settled()` 等待效应体达到稳定状态。

正常卸载时核心调用清理回调。连接中断时，网页宿主执行仍持有的本地清理，并对已完成及在途的清理回调去重。清理错误写入浏览器控制台，其他本地清理继续执行。连接断开后的回调应围绕 DOM、监听器与本地任务释放资源，远程服务的回收由核心处理。

页面提供的服务进入该页面的隔离域。显式命名域按 `(页面连接编号, 域名)` 区分，同页组件可以共享一个名称，两个页面使用相同名称时各自持有服务。后端注入沿部署配置的服务视图解析。页面关闭后，该页的子实例、效应和逻辑连接随所属页面一起释放。

必需提供方被撤回后，依赖它的页面子实例进入 `Pending`，核心执行效应清理，界面随之撤下。提供方插回后核心重新激活这些子实例，各页执行新的 `apply` 并取得当前服务 Client。

替换网页模块时，核心导出每页状态、清理旧实例，再从带替换序号的新入口地址加载模块并传入继承状态。全栈替换先激活新后端，再替换各页组件；候选激活失败时，事务恢复原后端与网页版本。`Frozen.check()` 供写入方法检查冻结状态，收到 `thaw` 时 SDK 清除冻结标记，再执行提供的业务恢复回调。

浏览器保留已导入的 ES 模块缓存，刷新页面可释放该文档的模块与内存。跨刷新数据适合保存在后端服务；`exportState` 服务于核心协调的加载代之间迁移。

## 运行记录

真实 Chrome 场景已经覆盖以下行为，运行入口见 [`tests/web-sdk`](../../../tests/web-sdk)：

| 场景 | 已观察的行为 | 证据目录，相对仓库根 |
|---|---|---|
| `pure-replace` | 两页文本与计数分别继承；坏 apply 恢复原模块；文档时间起点保持 | `target/web-m20/frontend/evidence/pure-replace-first` |
| `fullstack` | Rust 后端替换为 Python 0.2.0，存储内容与两页计数继承；坏前端候选使后端与页面共同恢复 | `target/web-m20/frontend/evidence/fullstack-first` |
| `http-restart` | 默认动态端口沿原地址恢复，令牌轮换，两页重连后完成按钮调用 | `target/web-m20/frontend/evidence/http-restart-default` |
| `realm-close` | 两页同名域计数保持独立；关闭一页后另一页继续调用并保持 context/revision | `target/web-m20/routing/evidence/realm-controls` |
| `dependencies` | 两页同时调用 Rust 存储、Python 文本规范化及 TypeScript 字符计数；三个提供方逐项撤回和插回，页面分别进入 Pending 并恢复；关闭一页后另一页继续调用 | `target/web-m20/frontend/evidence/dependencies-reconfigured` |
| `channel-faults` | 超大帧、畸形 CBOR、内层 channel 和真实洪泛使故障页关闭，健康页面继续调用 | `target/web-m20/routing/evidence/channel-faults` |
| `scaffold` | 生成的网页插件动态加入两页，替换时保留状态，移除时撤回 DOM、子实例和资源登记 | `target/web-m20/frontend/evidence/scaffold-first` |

规范与协议分别见 [DOC-15 §11](../../../docs/15-universal-socket.md#11-前端插件与全栈插件)、[DOC-16](../../../docs/16-up1-protocol.md)；部署和命名域实现见 [ADR-0041](../../../docs/adr/0041-routing-channels-web-deployment.md)。
