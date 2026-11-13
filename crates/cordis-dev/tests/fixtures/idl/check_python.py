import asyncio
import importlib.util
from pathlib import Path
import sys

GENERATED = Path(sys.argv[1])
sys.path.insert(0, sys.argv[2])
import cbor2


def load(name, path):
    specification = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(specification)
    sys.modules[name] = module
    specification.loader.exec_module(module)
    return module


types = load("wit_types", GENERATED / "types.py")
kv = load("wit_kv", GENERATED / "kv.py")


def serialize(value, module):
    def default(encoder, item):
        if isinstance(item, module.Tagged):
            encoder.encode(cbor2.CBORTag(item.tag, item.value))
        else:
            raise TypeError(type(item))
    return cbor2.dumps(value, default=default)


def deserialize(value, module):
    return cbor2.loads(value, tag_hook=lambda _, tag: module.Tagged(tag.tag, tag.value))


class Channel:
    """通过 CBOR 序列化连接生成的 Client 与真实提供方方法。"""

    def __init__(self, module, provider):
        self.module = module
        self.provider = provider

    async def invoke(self, interface, method, args, asynchronous):
        request = deserialize(serialize(args, self.module), self.module)
        result = await self.module.dispatch(self.provider, self, interface, method, request)
        return deserialize(serialize(result, self.module), self.module)


class Store:
    def __init__(self):
        self.values = {}

    def put(self, key, value):
        self.values[key] = value

    def get(self, key):
        return self.values.get(key)

    def keys(self):
        return sorted(self.values)

    async def flush(self):
        self.flushed = True


class Processor:
    def transform(self, value):
        value.label += ":python"
        value.coordinates = (value.coordinates[0] + 2, value.coordinates[1])
        return value

    def nested_values(self, values):
        return values + [types.Some(None)]

    def nested_maybe(self, values):
        return None if values is None else self.nested_values(values)

    def choose(self, value):
        if isinstance(value, types.ChoiceEmpty):
            return types.ChoiceNumber(17)
        return value


async def main():
    wire = deserialize((GENERATED / "rust-sample.cbor").read_bytes(), types)
    sample_spec = next(spec for spec in types.SCHEMA["types"].values() if spec.get("class") == "Sample")
    sample = types.decode_value(sample_spec, wire, None)
    assert sample.label == "数据" and sample.marker == "𝄞"
    assert sample.count == 2**64 - 1 and sample.delta == -(2**63)
    assert sample.bytes == [0, 128, 255] and sample.pairs == {"k": [7, 8]}
    assert sample.window == [4] * 40
    assert isinstance(sample.choice, types.ChoiceEmpty)
    assert sample.state == types.State.READY
    assert sample.permissions.read_value and not sample.permissions.write_value
    assert sample.nested == types.Some(None)
    assert sample.options == [None, types.Some(None), 9]
    assert sample.outcome == types.Err("business")
    provider = Processor()
    client = types.LaboratoryClient(Channel(types, provider))
    result = await client.transform(sample)
    assert result.label == "数据:python" and result.coordinates == (-1, 0.5)
    assert await client.nested_values([None, types.Some(None), 7]) == [None, types.Some(None), 7, types.Some(None)]
    assert await client.nested_maybe(None) is None
    assert await client.nested_maybe([types.Some(None)]) == [types.Some(None), types.Some(None)]
    assert await client.choose(types.ChoiceEmpty()) == types.ChoiceNumber(17)
    result_wire = types.encode_value(sample_spec, result, None)
    assert result_wire["nested"] == types.Tagged(60001, None)
    assert result_wire["choice"] == {"empty": None}
    assert result_wire["outcome"] == {"err": "business"}
    (GENERATED / "python-sample.cbor").write_bytes(serialize(result_wire, types))
    store = Store()
    client = kv.KvClient(Channel(kv, store))
    assert await client.get("missing") is None
    await client.put("a", bytes([0, 255]))
    assert await client.get("a") == bytes([0, 255])
    assert await client.keys() == ["a"]
    await client.flush()
    assert store.flushed
    print("Python: Rust CBOR 类型、嵌套 option、生成 Client/提供方分派、KV 字节调用通过")


asyncio.run(main())
