from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Protocol

from cordis_sdk.wit import Err, Ok, Some, Tagged, Transport, bind_runtime

import json
from enum import Enum
from typing import ClassVar

SCHEMA = json.loads("{\"package\":\"cordis:http@1.0.0\",\"services\":[{\"class\":\"Streams\",\"interface\":\"streams@1.0\",\"methods\":[]},{\"class\":\"CordisByteStreamStreamsByteStream\",\"interface\":\"cordis.byte-stream@1.0\",\"methods\":[{\"async\":true,\"error\":{\"kind\":\"string\"},\"name\":\"read\",\"params\":[{\"name\":\"max-bytes\",\"type\":{\"kind\":\"u32\"}}],\"python\":\"read\",\"result\":{\"ref\":\"3\"},\"since\":null,\"typescript\":\"read\"}]},{\"class\":\"Http\",\"interface\":\"cordis.http@1.0\",\"methods\":[{\"async\":true,\"error\":{\"kind\":\"string\"},\"name\":\"route\",\"params\":[{\"name\":\"spec\",\"type\":{\"ref\":\"12\"}},{\"name\":\"handler\",\"type\":{\"ref\":\"25\"}}],\"python\":\"route\",\"result\":{\"kind\":\"u64\"},\"since\":null,\"typescript\":\"route\"},{\"async\":true,\"error\":{\"kind\":\"string\"},\"name\":\"unroute\",\"params\":[{\"name\":\"id\",\"type\":{\"kind\":\"u64\"}}],\"python\":\"unroute\",\"result\":{\"kind\":\"unit\"},\"since\":null,\"typescript\":\"unroute\"}]},{\"class\":\"HttpHandler\",\"interface\":\"cordis.http-handler@1.0\",\"methods\":[{\"async\":true,\"error\":{\"kind\":\"string\"},\"name\":\"handle\",\"params\":[{\"name\":\"request\",\"type\":{\"ref\":\"15\"}}],\"python\":\"handle\",\"result\":{\"ref\":\"16\"},\"since\":null,\"typescript\":\"handle\"}]},{\"class\":\"Pages\",\"interface\":\"cordis.http-page@1.0\",\"methods\":[{\"async\":true,\"error\":{\"kind\":\"string\"},\"name\":\"attributes\",\"params\":[{\"name\":\"key\",\"type\":{\"ref\":\"9\"}}],\"python\":\"attributes\",\"result\":{\"ref\":\"23\"},\"since\":null,\"typescript\":\"attributes\"}]}],\"types\":{\"0\":{\"borrowed\":false,\"class\":\"CordisByteStreamStreamsByteStream\",\"kind\":\"resource\",\"service\":\"cordis.byte-stream@1.0\"},\"1\":{\"borrowed\":true,\"class\":\"CordisByteStreamStreamsByteStream\",\"kind\":\"resource\",\"service\":\"cordis.byte-stream@1.0\"},\"10\":{\"cases\":[{\"class\":\"BodyEmpty\",\"name\":\"empty\",\"type\":null},{\"class\":\"BodyInline\",\"name\":\"inline\",\"type\":{\"ref\":\"9\"}},{\"class\":\"BodyStream\",\"name\":\"stream\",\"type\":{\"ref\":\"8\"}}],\"class\":\"Body\",\"kind\":\"variant\"},\"11\":{\"item\":{\"kind\":\"string\"},\"kind\":\"list\"},\"12\":{\"class\":\"RouteSpec\",\"fields\":[{\"name\":\"prefix\",\"python\":\"prefix\",\"type\":{\"kind\":\"string\"},\"typescript\":\"prefix\"},{\"name\":\"methods\",\"python\":\"methods\",\"type\":{\"ref\":\"11\"},\"typescript\":\"methods\"},{\"name\":\"max-body-bytes\",\"python\":\"max_body_bytes\",\"type\":{\"ref\":\"6\"},\"typescript\":\"maxBodyBytes\"},{\"name\":\"timeout-ms\",\"python\":\"timeout_ms\",\"type\":{\"ref\":\"6\"},\"typescript\":\"timeoutMs\"}],\"kind\":\"record\"},\"13\":{\"items\":[{\"kind\":\"string\"},{\"kind\":\"string\"}],\"kind\":\"tuple\"},\"14\":{\"item\":{\"ref\":\"13\"},\"kind\":\"list\"},\"15\":{\"class\":\"HttpRequest\",\"fields\":[{\"name\":\"method\",\"python\":\"method\",\"type\":{\"kind\":\"string\"},\"typescript\":\"method\"},{\"name\":\"path\",\"python\":\"path\",\"type\":{\"kind\":\"string\"},\"typescript\":\"path\"},{\"name\":\"prefix\",\"python\":\"prefix\",\"type\":{\"kind\":\"string\"},\"typescript\":\"prefix\"},{\"name\":\"query\",\"python\":\"query\",\"type\":{\"kind\":\"string\"},\"typescript\":\"query\"},{\"name\":\"headers\",\"python\":\"headers\",\"type\":{\"ref\":\"14\"},\"typescript\":\"headers\"},{\"name\":\"peer\",\"python\":\"peer\",\"type\":{\"kind\":\"string\"},\"typescript\":\"peer\"},{\"name\":\"body\",\"python\":\"body\",\"type\":{\"ref\":\"10\"},\"typescript\":\"body\"}],\"kind\":\"record\"},\"16\":{\"class\":\"HttpResponse\",\"fields\":[{\"name\":\"status\",\"python\":\"status\",\"type\":{\"kind\":\"u16\"},\"typescript\":\"status\"},{\"name\":\"headers\",\"python\":\"headers\",\"type\":{\"ref\":\"14\"},\"typescript\":\"headers\"},{\"name\":\"body\",\"python\":\"body\",\"type\":{\"ref\":\"10\"},\"typescript\":\"body\"}],\"kind\":\"record\"},\"17\":{\"borrowed\":false,\"class\":\"HttpHandler\",\"kind\":\"resource\",\"service\":\"cordis.http-handler@1.0\"},\"18\":{\"borrowed\":true,\"class\":\"HttpHandler\",\"kind\":\"resource\",\"service\":\"cordis.http-handler@1.0\"},\"19\":{\"err\":{\"kind\":\"string\"},\"kind\":\"result\",\"ok\":{\"ref\":\"16\"}},\"2\":{\"item\":{\"kind\":\"u8\"},\"kind\":\"list\"},\"20\":{\"err\":{\"kind\":\"string\"},\"kind\":\"result\",\"ok\":{\"kind\":\"u64\"}},\"21\":{\"err\":{\"kind\":\"string\"},\"kind\":\"result\",\"ok\":{\"kind\":\"unit\"}},\"22\":{\"class\":\"PageAttributes\",\"fields\":[{\"name\":\"headers\",\"python\":\"headers\",\"type\":{\"ref\":\"14\"},\"typescript\":\"headers\"},{\"name\":\"peer\",\"python\":\"peer\",\"type\":{\"kind\":\"string\"},\"typescript\":\"peer\"}],\"kind\":\"record\"},\"23\":{\"item\":{\"ref\":\"22\"},\"kind\":\"option\"},\"24\":{\"err\":{\"kind\":\"string\"},\"kind\":\"result\",\"ok\":{\"ref\":\"23\"}},\"25\":{\"borrowed\":false,\"class\":\"HttpHandler\",\"kind\":\"resource\",\"service\":\"cordis.http-handler@1.0\"},\"3\":{\"item\":{\"ref\":\"2\"},\"kind\":\"option\"},\"4\":{\"err\":{\"kind\":\"string\"},\"kind\":\"result\",\"ok\":{\"ref\":\"3\"}},\"5\":{\"class\":\"CordisHttpHttpByteStream\",\"item\":{\"ref\":\"0\"},\"kind\":\"alias\"},\"6\":{\"item\":{\"kind\":\"u64\"},\"kind\":\"option\"},\"7\":{\"borrowed\":false,\"class\":\"CordisByteStreamStreamsByteStream\",\"kind\":\"resource\",\"service\":\"cordis.byte-stream@1.0\"},\"8\":{\"class\":\"StreamBody\",\"fields\":[{\"name\":\"length\",\"python\":\"length\",\"type\":{\"ref\":\"6\"},\"typescript\":\"length\"},{\"name\":\"source\",\"python\":\"source\",\"type\":{\"ref\":\"7\"},\"typescript\":\"source\"}],\"kind\":\"record\"},\"9\":{\"item\":{\"kind\":\"u8\"},\"kind\":\"list\"}}}")

class Streams(Protocol):
    pass

class StreamsClient:
    def __init__(self, transport: Transport):
        self._transport = transport

class CordisByteStreamStreamsByteStream(Protocol):
    async def read(self, max_bytes: int) -> bytes | None | Some[bytes]: ...

class CordisByteStreamStreamsByteStreamClient:
    def __init__(self, transport: Transport):
        self._transport = transport
    async def read(self, max_bytes: int) -> bytes | None | Some[bytes]:
        return await _invoke(self._transport, "cordis.byte-stream" + "@1.0", 0, [max_bytes])

class Http(Protocol):
    async def route(self, spec: RouteSpec, handler: HttpHandler) -> int: ...
    async def unroute(self, id: int) -> None: ...

class HttpClient:
    def __init__(self, transport: Transport):
        self._transport = transport
    async def route(self, spec: RouteSpec, handler: HttpHandler) -> int:
        return await _invoke(self._transport, "cordis.http" + "@1.0", 0, [spec, handler])
    async def unroute(self, id: int) -> None:
        return await _invoke(self._transport, "cordis.http" + "@1.0", 1, [id])

class HttpHandler(Protocol):
    async def handle(self, request: HttpRequest) -> HttpResponse: ...

class HttpHandlerClient:
    def __init__(self, transport: Transport):
        self._transport = transport
    async def handle(self, request: HttpRequest) -> HttpResponse:
        return await _invoke(self._transport, "cordis.http-handler" + "@1.0", 0, [request])

class Pages(Protocol):
    async def attributes(self, key: bytes) -> PageAttributes | None | Some[PageAttributes]: ...

class PagesClient:
    def __init__(self, transport: Transport):
        self._transport = transport
    async def attributes(self, key: bytes) -> PageAttributes | None | Some[PageAttributes]:
        return await _invoke(self._transport, "cordis.http-page" + "@1.0", 0, [key])

CordisHttpHttpByteStream = CordisByteStreamStreamsByteStream

@dataclass
class StreamBody:
    length: int | None | Some[int]
    source: CordisByteStreamStreamsByteStream

@dataclass
class BodyEmpty:
    kind: ClassVar[str] = "empty"
@dataclass
class BodyInline:
    kind: ClassVar[str] = "inline"
    value: bytes
@dataclass
class BodyStream:
    kind: ClassVar[str] = "stream"
    value: StreamBody
Body = BodyEmpty | BodyInline | BodyStream

@dataclass
class RouteSpec:
    prefix: str
    methods: list[str]
    max_body_bytes: int | None | Some[int]
    timeout_ms: int | None | Some[int]

@dataclass
class HttpRequest:
    method: str
    path: str
    prefix: str
    query: str
    headers: list[tuple[str, str]]
    peer: str
    body: Body

@dataclass
class HttpResponse:
    status: int
    headers: list[tuple[str, str]]
    body: Body

@dataclass
class PageAttributes:
    headers: list[tuple[str, str]]
    peer: str


_CLASSES = {"StreamBody": StreamBody, "BodyEmpty": BodyEmpty, "BodyInline": BodyInline, "BodyStream": BodyStream, "RouteSpec": RouteSpec, "HttpRequest": HttpRequest, "HttpResponse": HttpResponse, "PageAttributes": PageAttributes}
_CLIENTS = {"Streams": StreamsClient, "CordisByteStreamStreamsByteStream": CordisByteStreamStreamsByteStreamClient, "Http": HttpClient, "HttpHandler": HttpHandlerClient, "Pages": PagesClient}
encode_value, decode_value, _invoke, dispatch = bind_runtime(SCHEMA, _CLASSES, _CLIENTS)
