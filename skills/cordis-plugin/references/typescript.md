# TypeScript 进程插件

## 项目与构建

`cordis new-plugin hello --ts` 生成 src/main.ts、src/cordis_sdk、package.json、tsconfig.json 和进程清单。SDK 源码与入口共同编译到 dist，生成项目的 npm 配置使用普通文件依赖布局。

维护仓库 SDK 源码时，使用[仓库 SDK 构建](web.md#仓库-sdk-构建)中的 workspace 命令，一并更新 Node SDK、网页 bundle 与 HTTP 内置资源。下面的命令针对生成的用户插件，沿其 package.json 调用本地 TypeScript 入口。

```sh
npm --prefix plugins/hello install
npm --prefix plugins/hello run build
cordis run
```

清单的 command 为 `node dist/main.js`。标准输出承载 UP/1，诊断使用 `context.log` 或标准错误。应用入口从生成的 `./cordis_sdk/index.js` 导入 run；使用仓库 SDK 包的项目从 `cordis-sdk` 导入。

组件对象包含 name、apply(context, config) 与可选的 resolve(config)。inject 将接口标识映射为必需性；provides、listens 列出服务和事件；stateSchema、stateVersionMin、stateVersionMax 描述状态继承范围。

## 接口与引用

```sh
cordis idl gen interfaces/kv.wit --lang ts --output plugins/hello/src/bindings.ts
```

`await context.get("kv@1.0", binding, binding.KvClient)` 返回生成客户端；`await context.provide(interface, provider, binding)` 登记服务。方法返回普通值或 Promise，由生成绑定处理两者。`close(client)` 显式结束客户端份额；参数、结果和复合值中的资源引用由各次转交分别拥有。

资源事件使用 Context.on 的 binding 参数，回调通过 BindingTransport 和生成的 decodeValue 读取记录；Context.emit 使用同一 binding 的 encodeValue。普通持久状态采用值类型，服务资源留在其实际效应寿命内。

## 效应、取消与状态

`await context.effect(body, label)` 的 body 接收作用域 Context，在 `await scope.defer(cleanup)` 中登记回退。Effect.settled 等待体函数，Effect.dispose 等待回退，Effect.close 归还外部控制句柄。`context.task(signal => ...)` 把后台任务与 AbortSignal、结束等待绑定到同一效应。

`withSignal(signal, () => client.method(...))` 将使用方的取消信号传给核心。Node 提供方通过 currentSignal() 取得当前调用信号，交给支持 AbortSignal 的异步接口。生命周期中的 apply 与清理按核心调度完成。

`context.inherited` 使用 Map 表示原始 State，读取 schema、version、data；传入 apply 的 config 已转成普通对象。`context.exportState(schema, version, exporter, frozen, thaw)` 登记导出器，写入时调用 frozen.check。仓库 kv-ts 与 Rust KV 共用 kv.snapshot@1，entries 的字节值保存为整数数组。

`context.isolate(interface, realm)` 和 `context.intercept(interface, metadata)` 返回派生 Context；调用其 close() 归还外部视图份额。`context.plugin(component, config)` 装入当前插件描述符中的子组件。scoped 提供方的 scope(metadata) 返回保存该元数据的服务视图。
