# Python 插件

## 项目与入口

`cordis new-plugin hello --python` 生成 plugins/hello/main.py、cordis_sdk、requirements.txt 和 cordis-plugin.toml，并把插件目录登记到部署和 watch。入口将插件的 `.dependencies` 加入模块路径。

```sh
mkdir -p target/tmp
PIP_REQUIRE_VIRTUALENV=false TMPDIR="$PWD/target/tmp" \
  conda run -n base python -m pip install \
  --cache-dir "$PWD/target/python-sdk-dependencies/pip-cache" \
  --target plugins/hello/.dependencies -r plugins/hello/requirements.txt
cordis run
```

清单采用 `socket = "process"`、`protocol = 1`，command 为 `python -B main.py`。`-B` 使运行中的解释器保持安装副本的字节稳定。标准输出承载 UP/1，诊断使用 `await context.log(...)` 或标准错误。

依赖安装命令按 `--target` 写入当前插件，PIP_REQUIRE_VIRTUALENV 的设置作用于该次 Conda 命令。仓库测试使用 cordis_testkit::python::install_dependencies 共用相同的解释器、目标安装与工作区缓存规则。

`run(name, components)` 进入 asyncio 调度。`Component` 的 apply 接收 Context 与普通配置字典；inject 是接口标识到必需性的映射，provides 和 listens 声明服务及事件。提供状态时设置 state_schema、state_version_min、state_version_max。

## 生成接口与服务

```sh
cordis idl gen interfaces/kv.wit --lang python --output plugins/hello/bindings.py
```

导入整个生成模块作为 binding，再使用 `await context.get("kv@1.0", bindings, bindings.KvClient)`。远程 Client 的方法统一通过 await 调用；服务端通过 `await context.provide(interface, provider, bindings)` 登记。提供方方法可返回普通值或 awaitable，生成的 dispatch 按 WIT 编码参数与结果。

代理的 `_transport.close()` 显式归还服务引用，适合放在效应清理中。资源参数和结果使用生成的代理与 Tagged；事件包含资源时，on 的 binding 参数采用同一生成模块，通过 `context.import_resource(...)` 取得有类型代理，保留到回调之外的代理继续拥有独立引用。

## 效应与状态

`await context.effect(body, label, asynchronous=False)` 登记效应，body 接收带 scope 的 Context。`await scope.defer(cleanup)` 登记清理，cleanup 接收其清理 Context。`Effect.settled()` 等待体函数，`Effect.dispose()` 等待实际回退；后台协程使用 `context.task(function)`，由组件清理取消并等待结束。

`context.inherited` 是包含 schema、version、data 的状态字典。`await context.export_state(schema, version, exporter, frozen, thaw)` 登记导出器；导出前 Frozen 进入冻结状态，写入调用 `frozen.check()`，回退调用 thaw 并解除冻结。同步写入的冻结检查和状态修改放在同一临界区；跨服务写入按真实提交结果处理在途状态。

隔离视图使用 `await context.isolate(interface, realm)`，元数据视图使用 `await context.intercept(interface, metadata)`。scoped 提供方实现 scope(metadata) 返回绑定元数据的对象。`await context.plugin(component, config)` 装入同一插件描述符中的子组件，其资源随父级回收。

实际示例为仓库 plugins/kv-py：提供 kv@1.0，与 Rust KV 共用 kv.snapshot@1。数值、字节和状态的格式以同一份 WIT 及生成绑定为依据。
