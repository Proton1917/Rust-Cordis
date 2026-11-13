# DOC-16 §5–10：执行核心指令，持有对象和上下文，断开时回收资源。
import asyncio
import weakref

import cbor2

from .bindings import BindingTransport, Payload, Reference, Service
from .context import Callback, Context, Exporter, evaluate
from .errors import CordisError
from .wire import Connection


class Runtime:
    """DOC-16 §5–10：执行核心调度并持有当前连接的对象与上下文。"""

    def __init__(self, name, version, components, **limits):
        self.name = name
        self.version = version
        self.components = {component.name: component for component in components}
        self.exports = {}
        self.imports = weakref.WeakValueDictionary()
        self.contexts = {}
        self.events = {}
        self.next_object = 1
        self.connection = Connection(self, **limits)

    def description(self):
        return {
            "protocol": 1,
            "plugins": [
                {
                    "name": self.name,
                    "version": self.version,
                    "sdk_version": "0.1.0",
                    "build_info": "Python UP/1 SDK",
                    "format": 2,
                    "library": None,
                    "components": [
                        component.description()
                        for component in self.components.values()
                    ],
                }
            ],
        }

    def payload(self, value):
        return Payload(self, value)

    def reference(self, number):
        reference = self.imports.get(number)
        if reference is None:
            reference = Reference(self, number)
            self.imports[number] = reference
        return reference

    def offer(self, resource):
        if isinstance(resource, Service):
            resource.retain()
        number = self.next_object
        self.next_object += 1
        self.exports[number] = resource
        return number

    def release_exports(self, numbers):
        for number in numbers:
            resource = self.exports.pop(number, None)
            if isinstance(resource, Service):
                resource.close()

    def object(self, number):
        try:
            return self.exports[number]
        except KeyError as problem:
            raise CordisError(4, "对象已退役") from problem

    def outgoing(self, value, offers):
        """ADR-0039：生成标签转为 cbor2 标签，每次出口由发送事务持有。"""
        service = getattr(value, "_cordis_service", None)
        if service is not None:
            number = self.offer(service)
            offers.append(number)
            return cbor2.CBORTag(60000, number)
        if hasattr(value, "tag") and hasattr(value, "value"):
            if value.tag == 60000:
                raise CordisError(2, "资源标签需要由 export_resource 建立")
            return cbor2.CBORTag(value.tag, self.outgoing(value.value, offers))
        if isinstance(value, (list, tuple)):
            return [self.outgoing(item, offers) for item in value]
        if isinstance(value, dict):
            return {
                self.outgoing(key, offers): self.outgoing(item, offers)
                for key, item in value.items()
            }
        return value

    async def request(self, method, payload):
        """DOC-16 §8：按协议请求执行组件、服务与状态操作。"""
        params = payload.value
        if method == "hello":
            if params["protocol"] != 1:
                raise CordisError(3, "需要 UP/1 协议")
            return self.description(), []
        if method in ("resolve", "apply"):
            return await self._component(method, params), []
        if method == "invoke":
            return await self._invoke(params, payload)
        if method == "object_info":
            service = self.object(params["object"])
            return {
                "interface": service.interface,
                "methods": service.methods,
                "local": None,
            }, []
        if method == "callback":
            await self.callback(params, payload)
            return None, []
        if method in ("export", "thaw"):
            return await self._state(method, params), []
        if method == "shutdown":
            return None, []
        raise CordisError(3, f"请求方法缺席：{method}")

    async def _component(self, method, params):
        if params["plugin"] != self.name:
            raise CordisError(3, "插件名称与当前进程描述不符")
        component = self.components[params["component"]]
        if method == "resolve":
            if component.resolve is not None:
                await evaluate(component.resolve, params["config"])
            return None
        context = Context(
            self, params["context"], params["config"], params["inherited"]
        )
        self.contexts[context.number] = context
        await evaluate(component.apply, context, context.config)
        return None

    async def _invoke(self, params, payload):
        service = self.object(params["object"])
        if not isinstance(service, Service):
            raise CordisError(2, "invoke 对象需要服务类型")
        result = await service.invoke(self, params["method"], params["args"], payload)
        offers = []
        try:
            value = self.outgoing(result, offers)
            return {"value": value, "refs": offers}, offers
        except Exception:
            self.release_exports(offers)
            raise

    async def callback(self, params, payload):
        callback = self.object(params["object"])
        if not isinstance(callback, Callback):
            raise CordisError(2, "callback 对象需要回调类型")
        base = callback.context
        context = Context(
            self,
            params["context"],
            base.config,
            base.inherited,
            params["scope"],
            base.reference,
        )
        if callback.event:
            value = params["payload"]
            if callback.binding is not None:
                value = BindingTransport(self, callback.binding).incoming(
                    value, payload
                )
            await evaluate(callback.function, context, value)
        else:
            await evaluate(callback.function, context)

    async def _state(self, method, params):
        exporter = self.object(params["object"])
        if not isinstance(exporter, Exporter):
            raise CordisError(2, "状态操作需要导出器")
        if method == "thaw":
            exporter.frozen.frozen = False
            if exporter.thaw is not None:
                await evaluate(exporter.thaw)
            return None
        exporter.frozen.frozen = True
        try:
            value = await evaluate(exporter.function)
            return {"value": value}
        except Exception:
            exporter.frozen.frozen = False
            raise

    async def notification(self, method, params):
        if method == "release":
            self.release_exports(params["objects"])
        elif method == "cancel":
            entry = self.connection.running.get(params["id"])
            if entry is not None and entry[0] == "invoke":
                entry[1].cancel()
        elif method == "release_context":
            number = params["context"]
            self.contexts.pop(number, None)
            for task in self.events.pop(number, set()):
                task.cancel()
        elif method == "callback":
            payload = self.payload(params)
            task = asyncio.create_task(self._event(payload))
            tasks = self.events.setdefault(params["context"], set())
            tasks.add(task)
            task.add_done_callback(tasks.discard)
        else:
            raise CordisError(3, f"通知方法缺席：{method}")

    async def _event(self, payload):
        try:
            await self.callback(payload.value, payload)
        except Exception as problem:
            await self.connection.notify(
                "log",
                {
                    "context": payload.value["context"],
                    "level": 0,
                    "message": str(problem),
                },
            )
        finally:
            payload.close()

    def close(self):
        for tasks in self.events.values():
            for task in tasks:
                task.cancel()
        self.release_exports(list(self.exports))
        self.contexts.clear()
        self.imports.clear()


def run(name, components, version="0.1.0", **limits):
    """DOC-15 §10：进入进程插件标准流循环，EOF 后释放连接资源。"""
    asyncio.run(Runtime(name, version, components, **limits).connection.serve())
