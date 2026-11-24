# Cordis Python SDK

使用 Python 3.11 或更高版本编写进程插件。`cordis_sdk.run` 在 asyncio 事件循环中执行组件，核心通过 UP/1 决定激活与清理时机。标准输出传输协议帧，应用日志通过 `Context.log` 或标准错误发送。CBOR 编解码依赖 `cbor2==5.6.5`。

## 安装与第一个组件

进程清单使用 `python -B` 保持已安装插件目录的字节稳定，字节码缓存留在部署的编译与准备阶段。脚手架入口把 `.dependencies` 加入模块路径，依赖可安装在该插件目录。

```bash
python -m pip install ./sdks/python
cordis idl gen plugins/kv-api/kv.wit --lang python --output bindings.py
```

`--lang python` 将通用值编解码嵌入生成文件。运行目录已携带同版 Python SDK 时，可用 `--lang python-sdk` 让生成文件共用 `cordis_sdk.wit`。官方 Python KV、计数和 HTTP 插件采用该选项，各绑定的接口与类型图随所属插件分发。

Component 声明组件名称、apply、依赖、提供接口和状态模式。run 接受插件名称与组件列表，进入 asyncio 事件循环。核心在依赖满足时执行 apply(context, config)，组件通过 Context 贡献服务、效应和监听器。

```python
from cordis_sdk import Component, run
import bindings

async def apply(context, config):
    store = await context.get("kv@1.0", bindings, bindings.KvClient)
    await store.put("greeting", b"hello")

run("hello", [Component("hello", apply, inject={"kv@1.0": True})])
```

## 服务与效应

Context.provide(interface, provider, bindings, scoped=False) 使用生成模块的 dispatch 执行提供方方法。远程 Client 的方法均通过 await 调用。Context.service 可建立作为方法结果交出的服务资源；生成代理持有远程对象引用，最后一个 Python 引用释放时归还核心资源别名，也可通过代理的 _transport.close() 显式结束持有。

Context.effect(body, label, asynchronous=False) 向核心登记效应。body 接收带 scope 的 Context，通过 await scope.defer(cleanup) 登记回退；cleanup 接收清理时的 Context。asynchronous=True 的 Effect 可通过 settled() 等待体函数、dispose() 主动回退。Context.task(function) 把后台协程的取消和等待登记为效应清理。

Context.on(event, callback, asynchronous=False) 接收 callback(context, payload)。事件名包含版本，例如 kv.changed@1.0。Context.emit 等待核心接受事件，emit_nowait 供同步提供方提交通知。两种发送方式沿相同 UP/1 事件族派发。

携带资源的事件通过 `await context.on(event, callback, binding=bindings)` 登记。SDK 将资源标签转换为该生成模块的 Tagged，并保留当前载荷引用。回调通过 `context.import_resource(payload["item"], "resources.item@1.0", bindings, bindings.ItemClient)` 取得有类型代理；回调结束后代理继续持有资源。发送方可使用 `context.service(interface, provider, bindings)` 把服务资源放入 emit 的聚合载荷。转交远程资源的方法使用生成 schema 进行参数解码与结果编码，使各次转交分别保有引用。

## 替换与作用域

Context.export_state(schema, version, exporter, frozen, thaw) 登记状态导出器。导出之前 Frozen.frozen 设为 True；写操作调用 Frozen.check()，冻结期间得到 code 4。替换回退通过 thaw 解冻。组件在 context.inherited 读取 schema、version 和 data，并据状态契约重建业务状态。

await context.isolate(interface, realm) 派生隔离视图；realm 缺省时分配私有域。await context.intercept(interface, metadata) 合并拦截元数据。scoped 提供方实现 scope(metadata)，返回保存该元数据的视图对象。await context.plugin(component, config) 在当前视图装入同插件的子组件，其清理由父实例效应管理。

CordisError(code, message, detail) 保留 UP/1 错误通道和业务详情。请求截止时间与在途数量在 run 的 timeout、max_inflight 参数中设置，max_frame 控制单帧长度。invoke 支持调用取消；apply、效应体和清理按核心生命周期完成。

[Python 提供方示例](../../plugins/kv-py) · [部署和程序版本](../../docs/guide.md)

## UP/1 值图预算

`Runtime` 与 `run` 接受 `max_frame`、`max_depth`、`max_collection`、`max_values`。
默认值分别为 16777216 字节、64 层、65536 个元素和 262144 个节点。
深度包含消息包络；集合与节点分别按参数、成功结果或错误 detail 的值图计量。
输入在构造 CBOR 集合前检查声明规模，输出在编码前检查值图，编码缓冲按帧额度增长。
超限返回 code 8，detail 给出 `limit` 与 `maximum`；本次出口引用随编码失败归还。

共享边界向量位于 `tests/up1/value-budgets.tsv`。Python、TypeScript 和 Rust codec
分别通过 `test_budgets.py`、`shared-budgets.mjs` 和 `shared_budgets.rs` 使用同一输入。

## 异步通知回调容量

`Runtime` 与 `run` 接受 `max_active_callbacks` 和 `max_callback_bytes`，默认分别为
64 个与 67108864 字节。所有上下文的异步 callback 通知共享这两项连接级额度；请求式
callback 沿 `max_inflight` 管理。字节取读帧入口测得的完整 CBOR 通知正文长度，包含包络，
四字节帧头单独处理。该计量反映保留的线上载荷规模，Python 对象及用户另行复制的数据具有各自大小。

准入发生在创建执行任务前。超过任一额度会产生 code 8 并终止当前连接，核心由连接生命周期
回收其服务与效应。已接受通知沿接收顺序开始执行；异步完成顺序由回调决定。
`release_context` 发出取消后，任务及其载荷继续持有额度，实际完成或析构时归还；
首次执行前被取消的任务也完成相同清理。控制通知和反向调用响应可在额度占满时继续处理。
`runtime.callback_usage` 返回当前 `(任务数, 字节数)`。

直接调用 `Runtime.notification` 的嵌入方可传入第三个参数 `wire_bytes`，其值取自自身读帧入口；
缺省时按 `max_frame` 收取该回调额度。标准 UP/1 路径始终传入实际长度。
