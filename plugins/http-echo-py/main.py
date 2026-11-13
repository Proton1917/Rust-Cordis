import asyncio
import json
from pathlib import Path

from cordis_sdk import Component, CordisError, run
from cordis_sdk.http import register_route
from cordis_sdk.streams import copy_to, from_file, to_file

import bindings as api


def response(value):
    return api.HttpResponse(200, [("content-type", "application/json")], api.BodyInline(json.dumps(value).encode()))


class Handler:
    def __init__(self, directory, label):
        self.file = Path(directory) / "upload.bin"
        self.file.parent.mkdir(parents=True, exist_ok=True)
        self.label = label

    async def handle(self, request):
        path = request.path[len(request.prefix):]
        if path == "/file":
            if request.method in ("PUT", "POST"):
                if isinstance(request.body, api.BodyStream):
                    size = await to_file(request.body.value.source, self.file)
                else:
                    value = bytes(request.body.value) if isinstance(request.body, api.BodyInline) else b""
                    size = self.file.write_bytes(value)
                return response({"stored": size, "label": self.label})
            if request.method == "GET":
                return api.HttpResponse(200, [("content-type", "application/octet-stream")],
                    api.BodyStream(api.StreamBody(self.file.stat().st_size, from_file(self.file))))
            if request.method == "DELETE":
                self.file.unlink()
                return response({"removed": True})
        if path == "/error":
            raise CordisError(1, "HTTP 示例处理错误")
        if path == "/wait":
            await asyncio.sleep(60)
        if path == "/invalid-status":
            return api.HttpResponse(700, [], api.BodyEmpty())
        if path == "/version":
            return response({"label": self.label})
        body = bytearray()
        if isinstance(request.body, api.BodyInline):
            body.extend(request.body.value)
        elif isinstance(request.body, api.BodyStream):
            await copy_to(request.body.value.source, body.extend)
        result = response({"method": request.method, "path": request.path, "prefix": request.prefix,
            "query": request.query, "headers": request.headers, "peer": request.peer,
            "body": list(body), "label": self.label})
        result.headers.extend([("set-cookie", "session=one; HttpOnly; Secure"), ("set-cookie", "locale=zh; Secure"), ("connection", "keep-alive")])
        return result


async def apply(context, config):
    http = await context.get("cordis.http@1.0", api, api.HttpClient)
    directory = await context.get("cordis.data-dir@1.0", api)
    try:
        path = await directory.invoke("cordis.data-dir@1.0", "path", [], False)
    finally:
        directory.close()
    spec = api.RouteSpec(config.get("prefix", "/api"), config.get("methods", ["GET", "POST", "PUT", "DELETE"]),
        config.get("max_body_bytes"), config.get("timeout_ms"))
    await register_route(context, http, spec, Handler(path, config.get("label", "python")))


run("http-echo-py", [Component("http-echo", apply, inject={"cordis.http@1.0": True, "cordis.data-dir@1.0": True})])
