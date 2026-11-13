# DOC-15 §6.4、M18：组件登记、状态与派生视图统一经核心上下文操作。
import asyncio
import inspect
from dataclasses import dataclass, field

from .bindings import BindingTransport, Remote, Service
from .errors import CordisError


async def evaluate(function, *args):
    value = function(*args)
    return await value if inspect.isawaitable(value) else value


@dataclass
class Component:
    """DOC-16 §5、§7：组件声明交由核心解析依赖并调度 apply。"""

    name: str
    apply: object
    inject: dict = field(default_factory=dict)
    provides: tuple = ()
    listens: tuple = ()
    state_schema: str = ""
    state_version_min: int = 0
    state_version_max: int = 0
    resolve: object = None

    def description(self):
        return {
            "name": self.name,
            "inject": [
                {"interface": key, "required": required}
                for key, required in self.inject.items()
            ],
            "provides": list(self.provides),
            "listens": list(self.listens),
            "state_schema": self.state_schema,
            "state_version_min": self.state_version_min,
            "state_version_max": self.state_version_max,
        }


class Frozen:
    """DOC-11 §5.3：导出时冻结写入，替换回退时解冻。"""

    def __init__(self):
        self.frozen = False

    def check(self):
        if self.frozen:
            raise CordisError(4, "状态导出期间写入冻结")


@dataclass
class Callback:
    function: object
    context: object
    event: bool = False
    binding: object = None


@dataclass
class Exporter:
    function: object
    frozen: Frozen
    thaw: object = None


class Effect:
    """DOC-16 §8：外部句柄控制等待和回退，实际效应归核心持有。"""

    def __init__(self, runtime, number):
        self.runtime = runtime
        self.number = number

    async def settled(self):
        response = await self.runtime.connection.request(
            "effect_settled", {"effect": self.number}
        )
        response.close()

    async def dispose(self):
        response = await self.runtime.connection.request(
            "effect_dispose", {"effect": self.number}
        )
        response.close()

    def close(self):
        if self.number is not None:
            self.runtime.connection.notify_soon(
                "effect_release", {"effect": self.number}
            )
            self.number = None

    def __del__(self):
        self.close()


class Context:
    """DOC-15 §6.4、DOC-16 §7–8：每次加载和派生视图的插件操作入口。"""

    def __init__(
        self, runtime, number, config=None, inherited=None, scope=0, reference=None
    ):
        self.runtime = runtime
        self.number = number
        self.config = config
        self.inherited = inherited
        self.scope = scope
        self.reference = reference

    async def _request(self, method, **params):
        response = await self.runtime.connection.request(
            method, {"context": self.number, **params}
        )
        value = response.value
        response.close()
        return value

    async def _register(self, method, resource, field, **params):
        number = self.runtime.offer(resource)
        response = await self.runtime.connection.request(
            method,
            {
                "context": self.number,
                field: number,
                "refs": [number],
                **params,
            },
            [number],
        )
        value = response.value
        response.close()
        return value

    async def provide(self, interface, provider, binding, scoped=False):
        service = Service(provider, binding, interface, scoped)
        await self._register(
            "provide",
            service,
            "object",
            interface=interface,
            methods=service.methods,
            scoped=scoped,
        )

    async def get(self, interface, binding, client=None):
        response = await self.runtime.connection.request(
            "get", {"context": self.number, "interface": interface}
        )
        try:
            remote = Remote(
                self.runtime,
                binding,
                response.import_ref(response.value["object"]),
                interface,
            )
            return client(remote) if client else remote
        finally:
            response.close()

    def service(self, interface, provider, binding):
        return BindingTransport(self.runtime, binding).export_resource(
            provider, interface
        )

    def import_resource(self, value, interface, binding, client=None):
        """ADR-0039、DOC-16 §6：从事件载荷取得资源，代理保留独立引用。"""
        remote = BindingTransport(self.runtime, binding).import_resource(
            value, interface
        )
        return client(remote) if client else remote

    async def effect(self, body, label="python-effect", asynchronous=False):
        value = await self._register(
            "effect",
            Callback(body, self),
            "body",
            label=label,
            asynchronous=asynchronous,
        )
        return Effect(self.runtime, value["effect"])

    async def defer(self, cleanup, asynchronous=True):
        if self.scope == 0:
            raise CordisError(2, "defer 需要效应回调作用域")
        await self._register(
            "defer",
            Callback(cleanup, self),
            "callback",
            scope=self.scope,
            asynchronous=asynchronous,
        )

    async def on(self, event, callback, asynchronous=False, *, binding=None):
        await self._register(
            "on",
            Callback(callback, self, True, binding),
            "callback",
            event=event,
            asynchronous=asynchronous,
        )

    async def emit(self, event, payload=None, asynchronous=False):
        offers = []
        try:
            value = self.runtime.outgoing(payload, offers)
        except Exception:
            self.runtime.release_exports(offers)
            raise
        response = await self.runtime.connection.request(
            "emit",
            {
                "context": self.number,
                "event": event,
                "payload": value,
                "asynchronous": asynchronous,
                "refs": offers,
            },
            offers,
        )
        response.close()

    def emit_nowait(self, event, payload=None, asynchronous=False):
        """DOC-16 §8：同步提供方提交事件通知，发送故障结束对应连接。"""
        offers = []
        try:
            value = self.runtime.outgoing(payload, offers)
        except Exception:
            self.runtime.release_exports(offers)
            raise
        self.runtime.connection.notify_soon(
            "emit",
            {
                "context": self.number,
                "event": event,
                "payload": value,
                "asynchronous": asynchronous,
                "refs": offers,
            },
            offers,
        )

    async def export_state(self, schema, version, exporter, frozen=None, thaw=None):
        guard = frozen if frozen is not None else Frozen()
        await self._register(
            "export_state",
            Exporter(exporter, guard, thaw),
            "exporter",
            schema=schema,
            version=version,
        )
        return guard

    async def is_current(self):
        return await self._request("is_current")

    async def inherited_state(self):
        return await self._request("inherited_state")

    async def configuration(self):
        return (await self._request("config"))["value"]

    async def log(self, message, level=2):
        await self._request("log", level=level, message=message)

    async def derive(self, isolate=None, intercept=None):
        """DOC-16 §7：派生资源由 Context 和各项登记共同持有。"""
        response = await self.runtime.connection.request(
            "context_derive",
            {
                "context": self.number,
                "isolate": [
                    {"interface": key, "realm": realm}
                    for key, realm in (isolate or {}).items()
                ],
                "intercept": [
                    {"interface": key, "meta": meta}
                    for key, meta in (intercept or {}).items()
                ],
            },
        )
        try:
            return Context(
                self.runtime,
                response.value["context"],
                self.config,
                self.inherited,
                reference=response.import_ref(response.value["object"]),
            )
        finally:
            response.close()

    async def isolate(self, interface, realm=None):
        return await self.derive(isolate={interface: realm})

    async def intercept(self, interface, meta):
        return await self.derive(intercept={interface: meta})

    async def plugin(self, component, config=None):
        return (await self._request("plugin", component=component, config=config))[
            "instance"
        ]

    async def task(self, function, label="python-task"):
        """DOC-16 §8：后台协程由效应启动，清理负责取消与等待结束。"""

        async def body(scope):
            task = asyncio.create_task(function())

            async def cleanup(_context):
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass

            await scope.defer(cleanup)

        return await self.effect(body, label)
