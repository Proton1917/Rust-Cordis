# ADR-0039、ADR-0040：生成绑定与 UP/1 之间转交值和拥有型资源引用。
import inspect

import cbor2

from .errors import CordisError


class OwnedResource:
    """ADR-0046 §3.8：出口共享本地资源，最后一个远端持有者释放时关闭。"""

    def __init__(self):
        self._exports = 0

    def retain(self):
        self._exports += 1

    def release(self):
        self._exports -= 1
        if self._exports == 0:
            self.close()

    def close(self):
        raise NotImplementedError


class Reference:
    """DOC-16 §6：共享一份进口持有权，最后一个 Python 引用归还核心别名。"""

    def __init__(self, runtime, number):
        self.runtime = runtime
        self.number = number

    def __del__(self):
        self.runtime.connection.notify_soon("release", {"objects": [self.number]})


class Payload:
    """DOC-16 §6：每次接收持有 refs，解码出的代理独立保留对应引用。"""

    def __init__(self, runtime, value):
        self.value = value
        self.refs = {}
        for number in value.get("refs", []) if isinstance(value, dict) else []:
            if type(number) is not int or number <= 0 or number in self.refs:
                raise CordisError(2, "资源持有编号无效")
            self.refs[number] = runtime.reference(number)

    def import_ref(self, number):
        try:
            return self.refs[number]
        except KeyError as problem:
            raise CordisError(4, "载荷缺少资源持有权") from problem

    def close(self):
        self.refs.clear()


class Service:
    """ADR-0039：绑定提供方、生成模块和接口描述，执行统一 dispatch。"""

    def __init__(self, provider, binding, interface, scoped=False):
        self.provider = provider
        self.binding = binding
        self.interface = interface
        self.scoped = scoped
        service = next(
            item
            for item in binding.SCHEMA["services"]
            if item["interface"] == interface
        )
        self.definition = service
        self.methods = [method["name"] for method in service["methods"]]
        if scoped:
            self.methods.append("cordis.scope")

    async def invoke(self, runtime, method, args, payload):
        transport = BindingTransport(runtime, self.binding)
        if method == "cordis.scope" and self.scoped:
            if len(args) != 1:
                raise CordisError(2, "cordis.scope 需要一项元数据")
            view = self.provider.scope(args[0])
            if inspect.isawaitable(view):
                view = await view
            return transport.export_resource(view, self.interface, False)
        if method not in self.methods:
            raise CordisError(3, "接口方法缺席")
        values = transport.incoming(args, payload)
        if isinstance(self.provider, Remote):
            return await self._forward(transport, method, values)
        return await self.binding.dispatch(
            self.provider, transport, self.interface, method, values
        )

    def retain(self):
        if isinstance(self.provider, OwnedResource):
            self.provider.retain()

    def close(self):
        if isinstance(self.provider, OwnedResource):
            self.provider.release()

    async def _forward(self, transport, method, values):
        """ADR-0039：中转服务按生成类型导入再导出，建立各次转交的独立持有。"""
        index, description = next(
            (index, item)
            for index, item in enumerate(self.definition["methods"])
            if item["name"] == method
        )
        if len(values) != len(description["params"]):
            raise CordisError(2, "方法参数数量错误")
        decoded = [
            self.binding.decode_value(param["type"], value, transport, True)
            for param, value in zip(description["params"], values)
        ]
        result = await self.binding._invoke(
            self.provider, self.interface, index, decoded
        )
        return self.binding.encode_value(description["result"], result, transport, True)


class BindingTransport:
    """ADR-0039：生成 Client 与服务 dispatch 共用的标签和资源适配。"""

    def __init__(self, runtime, binding):
        self.runtime = runtime
        self.binding = binding

    def export_resource(self, value, interface, borrowed=False):
        provider = getattr(value, "_transport", value)
        if isinstance(provider, Remote):
            provider = Remote(
                provider.runtime,
                provider.binding,
                provider.reference,
                provider.interface,
            )
        tag = self.binding.Tagged(60000, 0)
        object.__setattr__(
            tag, "_cordis_service", Service(provider, self.binding, interface)
        )
        return tag

    def import_resource(self, value, interface, borrowed=False):
        return Remote(self.runtime, self.binding, value._cordis_reference, interface)

    def incoming(self, value, payload):
        if isinstance(value, cbor2.CBORTag):
            tag = self.binding.Tagged(value.tag, self.incoming(value.value, payload))
            if value.tag == 60000:
                object.__setattr__(
                    tag, "_cordis_reference", payload.import_ref(value.value)
                )
            return tag
        if isinstance(value, list):
            return [self.incoming(item, payload) for item in value]
        if isinstance(value, dict):
            return {
                self.incoming(key, payload): self.incoming(item, payload)
                for key, item in value.items()
            }
        return value


class Remote(BindingTransport):
    """DOC-16 §6：远程服务代理持有进口引用，调用结果沿生成模块解码。"""

    def __init__(self, runtime, binding, reference, interface):
        super().__init__(runtime, binding)
        self.reference = reference
        self.interface = interface

    async def invoke(self, interface, method, args, asynchronous=True):
        if self.reference is None:
            raise CordisError(4, "服务代理已关闭")
        offers = []
        try:
            values = self.runtime.outgoing(args, offers)
        except Exception:
            self.runtime.release_exports(offers)
            raise
        response = await self.runtime.connection.request(
            "invoke",
            {
                "object": self.reference.number,
                "method": method,
                "args": values,
                "refs": offers,
            },
            offers,
        )
        try:
            return self.incoming(response.value["value"], response)
        finally:
            response.close()

    def close(self):
        self.reference = None
