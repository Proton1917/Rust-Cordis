import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / ".dependencies"))

from cordis_sdk import Component, run


async def apply(context, config):
    """DOC-15 M18：组件激活和清理通过自身上下文记录。"""
    await context.log(config.get("message", "{{name}} 已激活"))

    async def start(scope):
        async def stop(cleanup_context):
            await cleanup_context.log("{{name}} 已清理")

        await scope.defer(stop)

    await context.effect(start, "{{name}}")


run("{{name}}", [Component("{{name}}", apply)])
