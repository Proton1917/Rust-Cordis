import asyncio

from cordis_sdk import Component, CordisError, run

from . import bindings


async def apply(context, config):
    """DOC-15 M18：经生成 Client 读取提供方，后台计数随效应停止。"""
    store = await context.get("kv@1.0", bindings, bindings.KvClient)
    key = config.get("key", "count")
    interval = config.get("interval_ms", 100) / 1000
    inherited = context.inherited
    if inherited is not None and (
        inherited["schema"] != "counter.state" or inherited["version"] != 1
    ):
        raise CordisError(2, "counter 需要 counter.state 版本 1")
    value = await store.get(key)
    if value is not None and len(value) != 8:
        raise CordisError(2, "counter 的存储值需要 8 字节")
    current = max(
        inherited["data"] if inherited else 0,
        int.from_bytes(value, "little") if value else 0,
    )
    frozen = await context.export_state("counter.state", 1, lambda: current)

    async def count():
        nonlocal current
        while True:
            if not frozen.frozen:
                if current >= 2**64 - 1:
                    raise CordisError(1, "counter 已达到 u64 上限")
                try:
                    await store.put(key, (current + 1).to_bytes(8, "little"))
                    current += 1
                except CordisError as problem:
                    if problem.code != 4:
                        raise
            await asyncio.sleep(interval)

    await context.task(count, "counter")


run(
    "counter",
    [
        Component(
            "counter",
            apply,
            inject={"kv@1.0": True},
            state_schema="counter.state",
            state_version_min=1,
            state_version_max=1,
        )
    ],
)
