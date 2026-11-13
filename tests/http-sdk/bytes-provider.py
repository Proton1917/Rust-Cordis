from cordis_sdk import Component, run
import bytes_binding as binding


class Provider:
    async def exchange(self, value):
        return value


async def apply(context, config):
    await context.provide("test.byte-payload@1.0", Provider(), binding)


run("byte-provider-py", [Component("bytes-provider", apply, provides=("test.byte-payload@1.0",))])
