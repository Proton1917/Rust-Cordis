# TypeScript 进程插件

## 项目与构建

`cordis new-plugin hello --ts` 生成 `src/main.ts`、`src/cordis_sdk`、`package.json`（`"type": "module"`）、`tsconfig.json` 和进程清单。SDK 源码与入口一起编译到 `dist`。

```sh
npm --prefix plugins/hello install
npm --prefix plugins/hello run build
cordis run
```

清单的 command 为 `node dist/main.js`。标准输出承载 UP/1，诊断用 `context.log` 或标准错误，不要 `console.log`。入口从生成的 `./cordis_sdk/index.js` 导入；以 npm 包方式使用 SDK 的项目从 `cordis-sdk` 导入。

## 最小完整示例

使用方组件：注入 `kv@1.0`，迁移自己的计数状态，在效应中归还客户端，运行后台任务。

```sh
cordis idl gen interfaces/kv.wit --lang ts --output plugins/visits/src/bindings.ts
```

```ts
import {setTimeout as sleep} from "node:timers/promises";
import {Context, CordisError, close, run} from "./cordis_sdk/index.js";
import * as binding from "./bindings.js";

async function apply(context: Context): Promise<void> {
    // 状态继承：先核对模式与版本，缺席时用初始值。
    const state = context.inherited as Map<string, any> | null;
    if (state && (state.get("schema") !== "visits" || state.get("version") !== 1)) {
        throw new CordisError(2, "需要 visits 版本 1");
    }
    const visits = {count: Number(state?.get("data").get("count") ?? 0)};
    const frozen = await context.exportState("visits", 1, () => ({count: visits.count}));
    frozen.check(); // 检查与修改之间没有 await，导出不会插进来
    visits.count += 1;

    // 必需依赖缺席时核心不会调用 apply。客户端在效应清理中归还。
    const kv = await context.get("kv@1.0", binding, binding.KvClient);
    const holder = await context.effect(async scope => {
        await scope.defer(() => close(kv));
    }, "kv-client");
    holder.close(); // 只归还外部句柄，效应仍由核心持有到实例停用

    // 后台任务：停用时 signal 中止，清理等待任务结束。
    await context.task(async signal => {
        while (!signal.aborted) {
            const keys = await kv.keys();
            await context.log(`共有 ${keys.length} 个键`, 3);
            await sleep(5000, undefined, {signal});
        }
    }, "poll-kv");
}

await run("visits", [{
    name: "visits",
    apply,
    inject: {"kv@1.0": true},
    stateSchema: "visits",
    stateVersionMin: 1,
    stateVersionMax: 1,
}]);
```

继承状态是 CBOR 解码后的 `Map`，嵌套字段用 `get` 读取。

## 组件与服务

组件对象含 `name`、`apply(context, config)`，可选 `resolve(config)`、`inject`（接口标识到“是否必需”）、`provides`、`listens`、`stateSchema`、`stateVersionMin`、`stateVersionMax`。

取服务：`await context.get(interface, binding, binding.XxxClient)` 返回生成的客户端，`close(client)` 归还引用。提供服务：`await context.provide(interface, provider, binding)`；第四个参数为 `true` 时为 scoped 提供方，`scope(metadata)` 返回保存该元数据的视图。方法可以返回普通值或 Promise。

事件载荷含资源时，`context.on(event, callback, asynchronous, binding)` 传入生成模块，回调中用生成的 `decodeValue` 读取；`context.emit` 用同一模块的 `encodeValue`。

## 效应、取消与状态

`await context.effect(body, label)` 的 body 接收作用域 Context，`await scope.defer(cleanup)` 登记清理。`Effect.settled` 等待体函数，`Effect.dispose` 等待清理，`Effect.close` 只归还外部句柄。`context.task(signal => ...)` 把后台任务与 AbortSignal 绑定到同一效应。

`withSignal(signal, () => client.method(...))` 把使用方的取消传到核心；提供方用 `currentSignal()` 取得本次调用的信号，交给支持 AbortSignal 的接口。

`context.exportState(schema, version, exporter, frozen?, thaw?)` 返回 Frozen，写入前调用 `frozen.check()`。`context.isolate(interface, realm)`、`context.intercept(interface, metadata)` 返回派生 Context，用完调用其 `close()`。`context.plugin(component, config)` 装入本插件描述符中的子组件。
