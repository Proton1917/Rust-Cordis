import json
from pathlib import Path

from cordis_sdk import Component, CordisError, run
from kv_py import bindings


async def exercise(context, config):
    remote = await context.get("kv@1.0", bindings)
    await context.defer(lambda _: remote.close())
    await remote.invoke("kv@1.0", "put", ["python-dynamic", bytes([0, 255])])
    value = await remote.invoke("kv@1.0", "get", ["python-dynamic"])
    assert value == bytes([0, 255])
    results = {"value": list(value)}
    for label, method, arguments, expected in [
        ("unknown_method", "absent-method", [], 3),
        ("forged_resource", "put", ["forged", bindings.Tagged(60000, 1)], 2),
    ]:
        try:
            await remote.invoke("kv@1.0", method, arguments)
        except CordisError as error:
            assert error.code == expected, error
            results[label] = error.code
        else:
            raise AssertionError(label + " 调用取得成功")
    Path(config["result"]).write_text(json.dumps(results))


async def apply(context, config):
    async def body(scope):
        await exercise(scope, config)
    effect = await context.effect(body, "dynamic-probe", asynchronous=True)
    await effect.settled()


run("dynamic-script-tests", [Component("probe", apply, inject={"kv@1.0": True})])
