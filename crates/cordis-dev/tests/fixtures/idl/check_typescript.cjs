const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const generated = path.resolve(process.argv[2]);
const cbor = require(path.resolve(process.argv[3], 'cbor-x'));
const types = require(path.join(generated, 'js/types.js'));
const kv = require(path.join(generated, 'js/kv.js'));

const encoder = new cbor.Encoder({ useRecords: false, mapsAsObjects: false, tagUint8Array: false });
const decoder = new cbor.Decoder({ mapsAsObjects: false });

// Transport 使用 CBOR 库的 Map、Uint8Array、bigint 与标签值。
function toCbor(value, module) {
    if (value instanceof module.Tagged) return new cbor.Tag(toCbor(value.value, module), value.tag);
    if (value instanceof Map) return new Map(Array.from(value, ([key, value]) => [toCbor(key, module), toCbor(value, module)]));
    if (Array.isArray(value)) return value.map((item) => toCbor(item, module));
    if (value && typeof value === 'object' && !(value instanceof Uint8Array)) {
        return new Map(Object.entries(value).map(([key, item]) => [key, toCbor(item, module)]));
    }
    return value;
}

function fromCbor(value, module) {
    if (value instanceof cbor.Tag) return new module.Tagged(value.tag, fromCbor(value.value, module));
    if (value instanceof Map) return new Map(Array.from(value, ([key, item]) => [fromCbor(key, module), fromCbor(item, module)]));
    if (Array.isArray(value)) return value.map((item) => fromCbor(item, module));
    return value;
}

class Channel {
    constructor(module, provider) { this.module = module; this.provider = provider; }
    async invoke(service, name, args, asynchronous) {
        const request = fromCbor(decoder.decode(encoder.encode(toCbor(args, this.module))), this.module);
        const result = await this.module.dispatch(this.provider, this, service, name, request);
        return fromCbor(decoder.decode(encoder.encode(toCbor(result, this.module))), this.module);
    }
}

async function main() {
    const spec = Object.values(types.SCHEMA.types).find((spec) => spec.class === 'Sample');
    const wire = fromCbor(decoder.decode(fs.readFileSync(path.join(generated, 'rust-sample.cbor'))), types);
    const sample = types.decodeValue(spec, wire, null);
    const shortCount = decoder.decode(Buffer.from([17]));
    assert.equal(types.decodeValue({kind: 'u64'}, shortCount, null), 17n);
    assert.equal(sample.label, '数据'); assert.equal(sample.marker, '𝄞');
    assert.equal(sample.count, 18446744073709551615n); assert.equal(sample.delta, -9223372036854775808n);
    assert.deepEqual(sample.bytes, [0, 128, 255]); assert.deepEqual(sample.pairs, new Map([['k', [7, 8]]]));
    assert.deepEqual(sample.window, Array(40).fill(4));
    assert.deepEqual(sample.choice, { kind: 'empty' }); assert.equal(sample.state, 'ready');
    assert.deepEqual(sample.permissions, { readValue: true, writeValue: false });
    assert.deepEqual(sample.nested, new types.Some(null));
    assert.deepEqual(sample.options, [null, new types.Some(null), 9]);
    assert.deepEqual(sample.outcome, { err: 'business' });
    const provider = {
        transform(value) { return { ...value, label: `${value.label}:typescript`, coordinates: [value.coordinates[0] + 3, value.coordinates[1]] }; },
        nestedValues(values) { return [...values, new types.Some(null)]; },
        nestedMaybe(values) { return values === null ? null : this.nestedValues(values); },
        choose(value) { return value.kind === 'empty' ? { kind: 'number', value: 17 } : value; },
    };
    const client = new types.LaboratoryClient(new Channel(types, provider));
    const result = await client.transform(sample);
    assert.equal(result.label, '数据:typescript'); assert.deepEqual(result.coordinates, [0, 0.5]);
    assert.deepEqual(await client.nestedValues([null, new types.Some(null), 7]), [null, new types.Some(null), 7, new types.Some(null)]);
    assert.equal(await client.nestedMaybe(null), null);
    assert.deepEqual(await client.nestedMaybe([new types.Some(null)]), [new types.Some(null), new types.Some(null)]);
    assert.deepEqual(await client.choose({ kind: 'empty' }), { kind: 'number', value: 17 });
    const resultWire = types.encodeValue(spec, result, null);
    assert.deepEqual(resultWire.get('nested'), new types.Tagged(60001, null));
    assert.deepEqual(resultWire.get('choice'), new Map([['empty', null]])); assert.deepEqual(resultWire.get('outcome'), new Map([['err', 'business']]));
    fs.writeFileSync(path.join(generated, 'typescript-sample.cbor'), encoder.encode(toCbor(resultWire, types)));
    const values = new Map(); let flushed = false;
    const store = { get(key) { return values.get(key) ?? null; }, put(key, value) { values.set(key, value); }, keys() { return [...values.keys()].sort(); }, async flush() { flushed = true; } };
    const kvClient = new kv.KvClient(new Channel(kv, store));
    assert.equal(await kvClient.get('missing'), null);
    await kvClient.put('a', new Uint8Array([0, 255]));
    assert.deepEqual(Array.from(await kvClient.get('a')), [0, 255]);
    assert.deepEqual(await kvClient.keys(), ['a']); await kvClient.flush(); assert.equal(flushed, true);
    console.log('TypeScript: Rust CBOR 类型、嵌套 option、生成 Client/提供方分派、KV 字节调用通过');
}

main().catch((error) => { console.error(error); process.exitCode = 1; });
