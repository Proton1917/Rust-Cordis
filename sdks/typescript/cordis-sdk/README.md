# TypeScript 进程插件 SDK

这个包用于在 Node 22 或更高版本中运行后端插件。组件通过标准输入输出与核心交换 UP/1 帧，核心协调依赖与生命周期。网页模块另用[浏览器宿主](../cordis-web/README.md)。

## 构建与启动

从 `sdks/typescript/` 执行 `npm ci --ignore-scripts` 和 `npm run build`，即可生成进程 SDK 和网页宿主。进程 SDK 使用 `cbor-x` 编解码，插件工程可通过本地 `file:` 依赖引用这个包。

```typescript
import {run} from "cordis-sdk";

await run("hello", [{
    name: "hello",
    async apply(context) {
        await context.log("hello 已激活");
        const effect = await context.effect(async scope => {
            const timer = setInterval(() => console.error("工作中"), 1000);
            await scope.defer(() => clearInterval(timer));
        });
        effect.close();
    },
}]);
```

组件对象包含 `name` 和 `apply(context, config)`。`inject` 将接口标识映射到必需性，`provides`、`listens` 列出服务和事件；`stateSchema`、`stateVersionMin`、`stateVersionMax` 描述继承范围。可选的 `resolve(config)` 在配置提交前检查业务规则。

## 调用服务与持有资源

`cordis idl gen --lang ts` 生成接口、数据类型和远程 Client。导入整个生成模块作为 binding，再通过 `await context.get("kv@1.0", binding, binding.KvClient)` 取得服务。提供方通过 `await context.provide("kv@1.0", provider, binding)` 登记。脚本提供方的结果可以是普通值或 Promise；传输始终按协议异步完成。

`close(client)` 归还客户端持有的服务份额。通过参数、结果和复合值转交服务时，SDK 创建独立出口引用，转交的代理具有各自的释放周期。回收后的 JavaScript 对象由 FinalizationRegistry 归还最后份额；显式 `close` 适用于需要确定释放时点的代码。

`context.on(event, callback, asynchronous, binding)` 接收事件中的资源标签，并为生成模块保留进口引用。callback 中以 `BindingTransport` 和生成的 `decodeValue` 解码事件记录；`context.emit` 接受生成 `encodeValue` 的结果，将资源和事件值一同交给核心。转交服务的方法参数或结果包含其他服务时，同样通过生成 Client 与 dispatch 完成各次引用转换。

`context.effect` 的 body 接收作用域 Context，`scope.defer` 登记清理回调。`Effect.settled()` 等待效应体，`Effect.dispose()` 等待实际回退，`Effect.close()` 归还外部控制句柄。后台异步任务通过 `context.task(signal => ...)` 运行，清理发送 AbortSignal 并等待任务结束。

`context.isolate(interface, realm)` 派生隔离视图，realm 省略时创建私有域；`context.intercept(interface, metadata)` 派生元数据视图。提供方启用 scoped 后实现 `scope(metadata)`，返回绑定元数据的提供方对象。`context.plugin(component, config)` 在当前视图挂载本插件的子组件，其清理由父级效应管理。

`context.exportState(schema, version, exporter, frozen, thaw)` 登记状态导出器。导出前 Frozen 进入冻结状态，写入方法调用 `frozen.check()`；回退时解除冻结并执行可选的 thaw。`context.inherited` 保存核心传来的 State，包含 schema、version 与 data。kv-ts 与 Rust KV 共用 kv.snapshot@1，entries 中的字节值保存为整数数组。

`withSignal(signal, () => client.method(...))` 将调用方取消传给核心。提供方通过 `currentSignal()` 取得当前调用的取消信号，将其传给 Node 的 timers、fetch 等异步接口。apply 和请求式清理按核心生命周期执行。标准输入结束后进程释放本地表项并退出。
