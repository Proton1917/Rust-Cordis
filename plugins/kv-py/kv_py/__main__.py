from cordis_sdk import Component, CordisError, Frozen, run

from . import bindings


class Store:
    """DOC-15 M18、AU-18：共享键值状态，元数据视图按 namespace 隔离键。"""

    def __init__(self, context, values, frozen, namespace=""):
        self.context = context
        self.values = values
        self.frozen = frozen
        self.namespace = namespace

    def scope(self, metadata):
        return Store(
            self.context, self.values, self.frozen, metadata.get("namespace", "")
        )

    def _key(self, key):
        return f"{self.namespace}/{key}" if self.namespace else key

    def get(self, key):
        return self.values.get(self._key(key))

    def put(self, key, value):
        self.frozen.check()
        self.values[self._key(key)] = value
        self.context.emit_nowait("kv.changed@1.0", {"key": key})

    def keys(self):
        prefix = f"{self.namespace}/" if self.namespace else ""
        return sorted(
            key[len(prefix) :] for key in self.values if key.startswith(prefix)
        )

    async def flush(self):
        pass


async def apply(context, config):
    """DOC-11 §5.3：接受 kv.snapshot@1，导出冻结与原生 KV 共享迁移格式。"""
    inherited = context.inherited
    if inherited is not None:
        if inherited["schema"] != "kv.snapshot" or inherited["version"] != 1:
            raise CordisError(2, "kv-store 需要 kv.snapshot 版本 1")
        values = {
            key: bytes(value) for key, value in inherited["data"]["entries"].items()
        }
    else:
        values = {}
    frozen = Frozen()
    store = Store(context, values, frozen)
    await context.export_state(
        "kv.snapshot",
        1,
        lambda: {"entries": {key: list(value) for key, value in values.items()}},
        frozen,
    )
    await context.provide("kv@1.0", store, bindings, scoped=True)


run(
    "kv",
    [
        Component(
            "kv-store",
            apply,
            provides=("kv@1.0",),
            state_schema="kv.snapshot",
            state_version_min=1,
            state_version_max=1,
        )
    ],
)
