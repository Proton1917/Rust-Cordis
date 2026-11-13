from cordis_sdk import Component, CordisError, run


async def apply(context, config):
    raise CordisError(1001, "候选组件激活失败")


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
