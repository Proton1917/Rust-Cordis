from __future__ import annotations

import inspect
from dataclasses import dataclass
from functools import partial
from typing import Any, Generic, Protocol, TypeVar

T = TypeVar("T")


@dataclass(frozen=True)
class Tagged:
    tag: int
    value: Any


@dataclass(frozen=True)
class Some(Generic[T]):
    value: T


@dataclass(frozen=True)
class Ok(Generic[T]):
    value: T


@dataclass(frozen=True)
class Err(Generic[T]):
    value: T


class Transport(Protocol):
    """ADR-0039：调用拥有导出引用，导入服务随客户端对象释放。"""

    async def invoke(self, interface: str, method: str, args: list[Any], asynchronous: bool) -> Any: ...
    def export_resource(self, value: Any, interface: str, borrowed: bool) -> Tagged: ...
    def import_resource(self, value: Tagged, interface: str, borrowed: bool) -> Transport: ...


@dataclass(frozen=True)
class _Binding:
    schema: dict
    classes: dict
    clients: dict
    services_by_interface: dict
    methods_by_service: dict


def bind_runtime(schema, classes, clients):
    services_by_interface = {service["interface"]: service for service in schema["services"]}
    methods_by_service = {
        service["interface"]: {method["name"]: method for method in service["methods"]}
        for service in schema["services"]
    }
    binding = _Binding(schema, classes, clients, services_by_interface, methods_by_service)
    return tuple(partial(function, binding) for function in (_encode, _decode, _invoke_method, _dispatch))


def _resolved(binding, spec):
    while "ref" in spec or spec.get("kind") == "alias":
        spec = binding.schema["types"][spec["ref"]] if "ref" in spec else spec["item"]
    return spec


def _scalar(kind, value):
    if kind == "unit":
        if value is not None:
            raise ValueError("unit 需要 null")
    elif kind == "bool":
        if type(value) is not bool:
            raise ValueError("bool 需要布尔值")
    elif kind in ("string", "char"):
        if not isinstance(value, str) or (kind == "char" and (len(value) != 1 or 0xD800 <= ord(value) <= 0xDFFF)):
            raise ValueError("string/char 需要有效 Unicode 文本")
    elif kind[0] in ("u", "s") and kind[1:].isdigit():
        bits = int(kind[1:])
        lower = 0 if kind[0] == "u" else -(1 << (bits - 1))
        upper = (1 << bits) if kind[0] == "u" else (1 << (bits - 1))
        if type(value) is not int or not lower <= value < upper:
            raise ValueError(f"{kind} 数值超出范围")
    elif kind in ("f32", "f64"):
        if type(value) not in (int, float):
            raise ValueError("浮点值需要数字")
        value = float(value)
    else:
        raise ValueError(f"未知 WIT 标量 {kind}")
    return value


def _encode(binding, spec, value, transport, method=False):
    """DOC-15 §7：语言值转换为 CBOR 库接收的标量、聚合或标签。"""
    spec = _resolved(binding, spec)
    kind = spec["kind"]
    if kind == "option":
        if value is None:
            return None
        inner = value.value if isinstance(value, Some) else value
        wire = _encode(binding, spec["item"], inner, transport, method)
        return Tagged(60001, wire) if wire is None or isinstance(wire, Tagged) and wire.tag == 60001 else wire
    if kind == "list":
        if _resolved(binding, spec["item"])["kind"] == "u8" and "length" not in spec:
            if not isinstance(value, bytes):
                raise ValueError("字节值需要 bytes")
            return value
        if "length" in spec and len(value) != spec["length"]:
            raise ValueError("固定数组长度错误")
        return [_encode(binding, spec["item"], item, transport) for item in value]
    if kind == "tuple":
        if len(value) != len(spec["items"]):
            raise ValueError("tuple 长度错误")
        return [_encode(binding, item, entry, transport) for item, entry in zip(spec["items"], value)]
    if kind == "map":
        return {_encode(binding, spec["key"], key, transport): _encode(binding, spec["item"], item, transport) for key, item in value.items()}
    if kind in ("record", "flags"):
        return {field["name"]: _encode(binding, field["type"], getattr(value, field["python"]), transport) for field in spec["fields"]}
    if kind == "variant":
        case = next(case for case in spec["cases"] if case["name"] == value.kind)
        return {case["name"]: None if case["type"] is None else _encode(binding, case["type"], value.value, transport)}
    if kind == "enum":
        return value.value
    if kind == "result":
        branch = "ok" if isinstance(value, Ok) else "err"
        return {branch: _encode(binding, spec[branch], value.value, transport)}
    if kind in ("resource", "error-context"):
        interface = spec.get("service", "cordis.error-context@1.0")
        return transport.export_resource(value, interface, spec.get("borrowed", False))
    return _scalar(kind, value)


def _decode(binding, spec, value, transport, method=False):
    """ADR-0039：导入资源获得当前调用的独立引用。"""
    spec = _resolved(binding, spec)
    kind = spec["kind"]
    if kind == "option":
        if value is None:
            return None
        inner = value.value if isinstance(value, Tagged) and value.tag == 60001 else value
        result = _decode(binding, spec["item"], inner, transport, method)
        return Some(result) if result is None or isinstance(result, Some) else result
    if kind == "list":
        if _resolved(binding, spec["item"])["kind"] == "u8" and "length" not in spec:
            if isinstance(value, bytes):
                return value
            if isinstance(value, list):
                return bytes(_scalar("u8", item) for item in value)
            raise ValueError("字节值需要字节串或 u8 数组")
        if not isinstance(value, list) or "length" in spec and len(value) != spec["length"]:
            raise ValueError("列表形状错误")
        return [_decode(binding, spec["item"], item, transport) for item in value]
    if kind == "tuple":
        if not isinstance(value, list) or len(value) != len(spec["items"]):
            raise ValueError("tuple 形状错误")
        return tuple(_decode(binding, item, entry, transport) for item, entry in zip(spec["items"], value))
    if kind == "map":
        return {_decode(binding, spec["key"], key, transport): _decode(binding, spec["item"], item, transport) for key, item in value.items()}
    if kind in ("record", "flags"):
        fields = {field["python"]: _decode(binding, field["type"], value[field["name"]], transport) for field in spec["fields"]}
        return binding.classes[spec["class"]](**fields)
    if kind == "variant":
        if not isinstance(value, dict) or len(value) != 1:
            raise ValueError("variant 需要单键映射")
        name, payload = next(iter(value.items()))
        case = next(case for case in spec["cases"] if case["name"] == name)
        if case["type"] is None:
            _scalar("unit", payload)
            return binding.classes[case["class"]]()
        return binding.classes[case["class"]](_decode(binding, case["type"], payload, transport))
    if kind == "enum":
        return binding.classes[spec["class"]](value)
    if kind == "result":
        if not isinstance(value, dict) or len(value) != 1 or next(iter(value)) not in ("ok", "err"):
            raise ValueError("result 需要 ok/err 单键映射")
        branch, payload = next(iter(value.items()))
        result = _decode(binding, spec[branch], payload, transport)
        return Ok(result) if branch == "ok" else Err(result)
    if kind in ("resource", "error-context"):
        if not isinstance(value, Tagged) or value.tag != 60000:
            raise ValueError("服务值需要对象引用标签")
        interface = spec.get("service", "cordis.error-context@1.0")
        return binding.clients[spec.get("class", "ErrorContext")](transport.import_resource(value, interface, spec.get("borrowed", False)))
    return _scalar(kind, value)


async def _invoke_method(binding, transport, interface, index, args):
    service = binding.services_by_interface[interface]
    method = service["methods"][index]
    values = [_encode(binding, param["type"], value, transport, True) for param, value in zip(method["params"], args)]
    result = await transport.invoke(interface, method["name"], values, method["async"])
    return _decode(binding, method["result"], result, transport, True)


async def _dispatch(binding, provider, transport, interface, name, args):
    """ADR-0039：提供方按声明异步属性执行，再编码统一成功值。"""
    methods = binding.methods_by_service.get(interface)
    if methods is None or name not in methods:
        raise StopIteration
    method = methods[name]
    if len(args) != len(method["params"]):
        raise ValueError("方法参数数量错误")
    values = [_decode(binding, param["type"], value, transport, True) for param, value in zip(method["params"], args)]
    result = getattr(provider, method["python"])(*values)
    if inspect.isawaitable(result):
        result = await result
    return _encode(binding, method["result"], result, transport, True)

