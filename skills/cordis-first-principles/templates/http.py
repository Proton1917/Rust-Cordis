import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / ".dependencies"))

from cordis_sdk import Component, run
from cordis_sdk.http import register_route
import http_bindings as api


class Handler:
    async def handle(self, request):
        return api.HttpResponse(200, [("content-type", "text/plain; charset=utf-8")],
            api.BodyInline(f"{request.method} {request.path}\n".encode()))


async def apply(context, config):
    http = await context.get("cordis.http@1.0", api, api.HttpClient)
    await register_route(context, http, api.RouteSpec("/{{name}}", [], None, None), Handler())


run("{{name}}", [Component("{{name}}", apply, inject={"cordis.http@1.0": True})])
