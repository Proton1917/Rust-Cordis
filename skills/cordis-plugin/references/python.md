# Python 插件

## 项目与依赖

`cordis new-plugin hello --python` 生成 `plugins/hello/main.py`、`cordis_sdk`、`requirements.txt` 和 `cordis-plugin.toml`，并把插件登记到部署和 watch。入口把插件目录下的 `.dependencies` 加入模块路径，依赖装在这里：

```sh
python -m pip install --target plugins/hello/.dependencies -r plugins/hello/requirements.txt
cordis run
```

`python` 取项目约定的解释器（3.11 及以上）。使用 Conda 的环境写成 `conda run -n <环境> python -m pip ...`；环境设置了 `PIP_REQUIRE_VIRTUALENV` 时，对这次命令设为 `false`。

清单为 `socket = "process"`、`protocol = 1`，command 为 `python -B main.py`；`-B` 让运行中的解释器不写字节码，保持安装副本不变。标准输出承载 UP/1，诊断用 `await context.log(...)` 或标准错误，不要 `print`。

## 最小完整示例

使用方组件：注入 `kv@1.0`，迁移自己的计数状态，在效应中归还客户端，运行后台协程。

```sh
cordis idl gen interfaces/kv.wit --lang python --output plugins/visits/bindings.py
```

```python
import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / ".dependencies"))

from cordis_sdk import Component, CordisError, run

import bindings


async def apply(context, config):
    # 状态继承：先核对模式与版本，缺席时用初始值。
    state = context.inherited
    if state is not None and (state["schema"] != "visits" or state["version"] != 1):
        raise CordisError(2, "需要 visits 版本 1")
    visits = {"count": state["data"]["count"] if state else 0}
    frozen = await context.export_state("visits", 1, lambda: dict(visits))
    frozen.check()  # 检查与修改之间没有 await，导出不会插进来
    visits["count"] += 1

    # 必需依赖缺席时核心不会调用 apply。Remote 在效应清理中归还。
    remote = await context.get("kv@1.0", bindings)
    kv = bindings.KvClient(remote)

    async def hold(scope):
        async def release(_cleanup_context):
            remote.close()

        await scope.defer(release)

    await context.effect(hold, "kv-client")

    # 后台协程：停用时被取消，清理等待它结束。
    async def poll():
        while True:
            keys = await kv.keys()
            await context.log(f"共有 {len(keys)} 个键", level=3)
            await asyncio.sleep(5)

    await context.task(poll, "poll-kv")


run(
    "visits",
    [
        Component(
            "visits",
            apply,
            inject={"kv@1.0": True},
            state_schema="visits",
            state_version_min=1,
            state_version_max=1,
        )
    ],
)
```

## 组件与服务

`run(name, components)` 进入 asyncio 调度。`Component(name, apply, inject=..., provides=..., listens=..., state_schema=..., state_version_min=..., state_version_max=..., resolve=...)`；inject 是接口标识到“是否必需”的映射，apply 接收 Context 与普通配置字典。

取服务：`remote = await context.get(interface, bindings)` 返回 Remote，用生成的 `bindings.XxxClient(remote)` 包装；也可以 `await context.get(interface, bindings, bindings.XxxClient)` 直接取客户端。Remote 的 `close()` 归还服务引用，放在效应清理中。客户端方法一律 `await`。

提供服务：`await context.provide(interface, provider, bindings)`。提供方方法可以返回普通值或 awaitable，生成的分派按 WIT 编码参数与结果。提供方实现 `scope(metadata)` 并传 `scoped=True` 时，为每个使用方的拦截元数据返回独立视图。

资源参数与结果用生成的代理；事件载荷含资源时，`on` 传入同一个 bindings 模块，在回调中用 `context.import_resource(...)` 取得有类型的代理。

## 效应与状态

`await context.effect(body, label)` 登记效应，body 接收作用域 Context；`await scope.defer(cleanup)` 登记清理，cleanup 接收清理用的 Context。`Effect.settled()` 等待体函数完成，`Effect.dispose()` 等待实际清理。`context.task(function, label)` 启动后台协程，清理时取消并等待。

`context.inherited` 是含 `schema`、`version`、`data` 的字典，没有继承状态时为 `None`。`await context.export_state(schema, version, exporter, frozen=None, thaw=None)` 返回 Frozen；写入前调用 `frozen.check()`，检查与修改之间不要有 `await`。跨服务写入要按真实提交结果处理在途状态，见[资源与状态](resources-state.md)。

隔离视图 `await context.isolate(interface, realm)`，元数据视图 `await context.intercept(interface, metadata)`，子组件 `await context.plugin(component, config)`，子组件随父级回收。
