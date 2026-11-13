import asyncio
from dataclasses import asdict
import json
from pathlib import Path

from cordis_sdk import Component, run
import http_binding as api


async def apply(context, config):
    service = await context.get("cordis.http-page@1.0", api, api.PagesClient)
    commands, results = Path(config["commands"]), Path(config["results"])
    commands.mkdir(parents=True, exist_ok=True)
    results.mkdir(parents=True, exist_ok=True)

    async def work():
        while True:
            for path in sorted(commands.glob("*.json")):
                request = json.loads(path.read_text())
                path.unlink()
                value = await service.attributes(bytes(request["key"]))
                temporary = results / (path.stem + ".next")
                temporary.write_text(json.dumps(asdict(value) if value is not None else None))
                temporary.rename(results / path.name)
            await asyncio.sleep(0.01)

    await context.task(work, "page-attributes-inspector")


run("page-inspector", [Component("inspector", apply, inject={"cordis.http-page@1.0": True})])
