# Cordis Python SDK

Python 插件由核心通过 UP/1 标准流连接调度。Python 3.11 及以上可安装本目录的软件包，运行依赖为 cbor2 5.6.5。进程标准输出承载协议，诊断通过 Context.log 或标准错误输出发送。

进程清单使用 `python -B` 保持已安装插件目录的字节稳定，字节码缓存留在部署的编译与准备阶段。脚手架入口把 `.dependencies` 加入模块路径，依赖可安装在该插件目录。

```bash
python -m pip install ./sdks/python
cordis idl gen plugins/kv-api/kv.wit --lang python --output bindings.py
```

Component 声明组件名称、apply、依赖、提供接口和状态模式。run 接受插件名称与组件列表，进入 asyncio 事件循环。核心在依赖满足时执行 apply(context, config)，组件通过 Context 贡献服务、效应和监听器。

```python
from cordis_sdk import Component, run
import bindings

async def apply(context, config):
    store = await context.get("kv@1.0", bindings, bindings.KvClient)
    await store.put("greeting", b"hello")

run("hello", [Component("hello", apply, inject={"kv@1.0": True})])
```

Context.provide(interface, provider, bindings, scoped=False) 使用生成模块的 dispatch 执行提供方方法。远程 Client 的方法均通过 await 调用。Context.service 可建立作为方法结果交出的服务资源；生成代理持有远程对象引用，最后一个 Python 引用释放时归还核心资源别名，也可通过代理的 _transport.close() 显式结束持有。

Context.effect(body, label, asynchronous=False) 向核心登记效应。body 接收带 scope 的 Context，通过 await scope.defer(cleanup) 登记回退；cleanup 接收清理时的 Context。asynchronous=True 的 Effect 可通过 settled() 等待体函数、dispose() 主动回退。Context.task(function) 把后台协程的取消和等待登记为效应清理。

Context.on(event, callback, asynchronous=False) 接收 callback(context, payload)。事件名包含版本，例如 kv.changed@1.0。Context.emit 等待核心接受事件，emit_nowait 供同步提供方提交通知。两种发送方式沿相同 UP/1 事件族派发。

携带资源的事件通过 `await context.on(event, callback, binding=bindings)` 登记。SDK 将资源标签转换为该生成模块的 Tagged，并保留当前载荷引用。回调通过 `context.import_resource(payload["item"], "resources.item@1.0", bindings, bindings.ItemClient)` 取得有类型代理；回调结束后代理继续持有资源。发送方可使用 `context.service(interface, provider, bindings)` 把服务资源放入 emit 的聚合载荷。转交远程资源的方法使用生成 schema 进行参数解码与结果编码，使各次转交分别保有引用。

Context.export_state(schema, version, exporter, frozen, thaw) 登记状态导出器。导出之前 Frozen.frozen 设为 True；写操作调用 Frozen.check()，冻结期间得到 code 4。替换回退通过 thaw 解冻。组件在 context.inherited 读取 schema、version 和 data，并据状态契约重建业务状态。

await context.isolate(interface, realm) 派生隔离视图；realm 缺省时分配私有域。await context.intercept(interface, metadata) 合并拦截元数据。scoped 提供方实现 scope(metadata)，返回保存该元数据的视图对象。await context.plugin(component, config) 在当前视图装入同插件的子组件，其清理由父实例效应管理。

CordisError(code, message, detail) 保留 UP/1 错误通道和业务详情。请求截止时间与在途数量在 run 的 timeout、max_inflight 参数中设置，max_frame 控制单帧长度。invoke 支持调用取消；apply、效应体和清理按核心生命周期完成。
