from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Protocol

from cordis_sdk.wit import Err, Ok, Some, Tagged, Transport, bind_runtime

import json
from enum import Enum
from typing import ClassVar

SCHEMA = json.loads("{\"package\":\"cordis:kv@1.0.0\",\"services\":[{\"class\":\"Kv\",\"interface\":\"kv@1.0\",\"methods\":[{\"async\":false,\"error\":{\"kind\":\"string\"},\"name\":\"get\",\"params\":[{\"name\":\"key\",\"type\":{\"kind\":\"string\"}}],\"python\":\"get\",\"result\":{\"ref\":\"4\"},\"since\":null,\"typescript\":\"get\"},{\"async\":false,\"error\":{\"kind\":\"string\"},\"name\":\"put\",\"params\":[{\"name\":\"key\",\"type\":{\"kind\":\"string\"}},{\"name\":\"value\",\"type\":{\"ref\":\"0\"}}],\"python\":\"put\",\"result\":{\"kind\":\"unit\"},\"since\":null,\"typescript\":\"put\"},{\"async\":false,\"error\":{\"kind\":\"string\"},\"name\":\"keys\",\"params\":[],\"python\":\"keys\",\"result\":{\"ref\":\"7\"},\"since\":null,\"typescript\":\"keys\"},{\"async\":true,\"error\":{\"kind\":\"string\"},\"name\":\"flush\",\"params\":[],\"python\":\"flush\",\"result\":{\"kind\":\"unit\"},\"since\":null,\"typescript\":\"flush\"}]}],\"types\":{\"0\":{\"item\":{\"kind\":\"u8\"},\"kind\":\"list\"},\"1\":{\"item\":{\"ref\":\"0\"},\"key\":{\"kind\":\"string\"},\"kind\":\"map\"},\"2\":{\"class\":\"Snapshot\",\"fields\":[{\"name\":\"entries\",\"python\":\"entries\",\"type\":{\"ref\":\"1\"},\"typescript\":\"entries\"}],\"kind\":\"record\"},\"3\":{\"class\":\"Changed\",\"fields\":[{\"name\":\"key\",\"python\":\"key\",\"type\":{\"kind\":\"string\"},\"typescript\":\"key\"}],\"kind\":\"record\"},\"4\":{\"item\":{\"ref\":\"0\"},\"kind\":\"option\"},\"5\":{\"err\":{\"kind\":\"string\"},\"kind\":\"result\",\"ok\":{\"ref\":\"4\"}},\"6\":{\"err\":{\"kind\":\"string\"},\"kind\":\"result\",\"ok\":{\"kind\":\"unit\"}},\"7\":{\"item\":{\"kind\":\"string\"},\"kind\":\"list\"},\"8\":{\"err\":{\"kind\":\"string\"},\"kind\":\"result\",\"ok\":{\"ref\":\"7\"}}}}")

class Kv(Protocol):
    def get(self, key: str) -> bytes | None | Some[bytes]: ...
    def put(self, key: str, value: bytes) -> None: ...
    def keys(self) -> list[str]: ...
    async def flush(self) -> None: ...

class KvClient:
    def __init__(self, transport: Transport):
        self._transport = transport
    async def get(self, key: str) -> bytes | None | Some[bytes]:
        return await _invoke(self._transport, "kv" + "@1.0", 0, [key])
    async def put(self, key: str, value: bytes) -> None:
        return await _invoke(self._transport, "kv" + "@1.0", 1, [key, value])
    async def keys(self) -> list[str]:
        return await _invoke(self._transport, "kv" + "@1.0", 2, [])
    async def flush(self) -> None:
        return await _invoke(self._transport, "kv" + "@1.0", 3, [])

@dataclass
class Snapshot:
    entries: dict[str, bytes]

@dataclass
class Changed:
    key: str


_CLASSES = {"Snapshot": Snapshot, "Changed": Changed}
_CLIENTS = {"Kv": KvClient}
encode_value, decode_value, _invoke, dispatch = bind_runtime(SCHEMA, _CLASSES, _CLIENTS)
