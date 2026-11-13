// DOC-15 §7、ADR-0039：绑定以显式 Transport 管理调用与资源引用。
export class Tagged {
    constructor(readonly tag: number, readonly value: unknown) {}
}

export class Some<T> {
    constructor(readonly value: T) {}
}

export type Option<T> = T | null | Some<T>;
export type ResultValue<T, E> = { ok: T } | { err: E };

export interface Transport {
    invoke(service: string, method: string, args: unknown[], asynchronous: boolean): Promise<unknown>;
    exportResource(value: unknown, service: string, borrowed: boolean): Tagged;
    importResource(value: Tagged, service: string, borrowed: boolean): Transport;
}

type Spec = Record<string, any>;
interface Method {
    name: string;
    python: string;
    typescript: string;
    async: boolean;
    since: string | null;
    params: Array<{ name: string; type: Spec }>;
    result: Spec;
    error: Spec | null;
}
interface Service { class: string; interface: string; methods: Method[] }
interface Schema { package: string; types: Record<string, Spec>; services: Service[] }

function resolved(input: Spec): Spec {
    let spec = input;
    while ("ref" in spec || spec.kind === "alias") {
        spec = "ref" in spec ? SCHEMA.types[spec.ref] : spec.item;
    }
    return spec;
}

function scalar(kind: string, value: unknown): unknown {
    if (kind === "unit") {
        if (value !== null && value !== undefined) throw new TypeError("unit 需要 null");
        return null;
    }
    if (kind === "bool") {
        if (typeof value !== "boolean") throw new TypeError("bool 需要布尔值");
    } else if (kind === "string" || kind === "char") {
        if (typeof value !== "string") throw new TypeError("string/char 需要文本");
        const code = value.codePointAt(0) ?? 0;
        if (kind === "char" && (Array.from(value).length !== 1 || code >= 0xd800 && code <= 0xdfff)) {
            throw new TypeError("char 需要 Unicode 标量");
        }
    } else if (/^[us](8|16|32|64)$/.test(kind)) {
        const bits = Number(kind.slice(1));
        const lower = kind[0] === "u" ? 0n : -(1n << BigInt(bits - 1));
        const upper = 1n << BigInt(kind[0] === "u" ? bits : bits - 1);
        if (bits === 64) {
            if (typeof value !== "bigint" || value < lower || value >= upper) throw new TypeError(`${kind} 数值超出范围`);
        } else if (typeof value !== "number" || !Number.isInteger(value) || value < Number(lower) || value >= Number(upper)) {
            throw new TypeError(`${kind} 数值超出范围`);
        }
    } else if (kind === "f32" || kind === "f64") {
        if (typeof value !== "number") throw new TypeError("浮点值需要数字");
    } else {
        throw new TypeError(`未知 WIT 标量 ${kind}`);
    }
    return value;
}

export function encodeValue(input: Spec, value: any, transport: Transport, method = false): unknown {
    const spec = resolved(input);
    switch (spec.kind) {
        case "option": {
            if (value === null) return null;
            const inner = value instanceof Some ? value.value : value;
            const wire = encodeValue(spec.item, inner, transport, method);
            return wire === null || wire instanceof Tagged && wire.tag === 60001 ? new Tagged(60001, wire) : wire;
        }
        case "list":
            if (method && resolved(spec.item).kind === "u8" && spec.length === undefined) {
                if (!(value instanceof Uint8Array)) throw new TypeError("方法字节值需要 Uint8Array");
                return value;
            }
            if (!Array.isArray(value) || spec.length !== undefined && value.length !== spec.length) throw new TypeError("列表形状错误");
            return value.map((item) => encodeValue(spec.item, item, transport));
        case "tuple":
            if (!Array.isArray(value) || value.length !== spec.items.length) throw new TypeError("tuple 长度错误");
            return value.map((item, index) => encodeValue(spec.items[index], item, transport));
        case "map":
            if (!(value instanceof Map)) throw new TypeError("映射需要 Map");
            return new Map(Array.from(value, ([key, item]) => [encodeValue(spec.key, key, transport), encodeValue(spec.item, item, transport)]));
        case "record":
        case "flags":
            return new Map(spec.fields.map((field: Spec) => [field.name, encodeValue(field.type, value[field.typescript], transport)]));
        case "variant": {
            const entry = spec.cases.find((entry: Spec) => entry.name === value.kind);
            if (!entry) throw new TypeError("variant 分支错误");
            return new Map([[entry.name, entry.type === null ? null : encodeValue(entry.type, value.value, transport)]]);
        }
        case "enum":
            if (!spec.cases.includes(value)) throw new TypeError("enum 值错误");
            return value;
        case "result": {
            const branch = "ok" in value ? "ok" : "err";
            return new Map([[branch, encodeValue(spec[branch], value[branch], transport)]]);
        }
        case "resource":
        case "error-context":
            return transport.exportResource(value, spec.service ?? "cordis.error-context@1.0", spec.borrowed ?? false);
        default:
            return scalar(spec.kind, value);
    }
}

export function decodeValue(input: Spec, value: any, transport: Transport, method = false): any {
    const spec = resolved(input);
    switch (spec.kind) {
        case "option": {
            if (value === null) return null;
            const inner = value instanceof Tagged && value.tag === 60001 ? value.value : value;
            const result = decodeValue(spec.item, inner, transport, method);
            return result === null || result instanceof Some ? new Some(result) : result;
        }
        case "list":
            if (method && resolved(spec.item).kind === "u8" && spec.length === undefined) {
                if (!(value instanceof Uint8Array)) throw new TypeError("方法字节值需要 CBOR 字节串");
                return value;
            }
            if (!Array.isArray(value) || spec.length !== undefined && value.length !== spec.length) throw new TypeError("列表形状错误");
            return value.map((item) => decodeValue(spec.item, item, transport));
        case "tuple":
            if (!Array.isArray(value) || value.length !== spec.items.length) throw new TypeError("tuple 形状错误");
            return value.map((item, index) => decodeValue(spec.items[index], item, transport));
        case "map":
            if (!(value instanceof Map)) throw new TypeError("映射需要 CBOR 映射");
            return new Map(Array.from(value, ([key, item]) => [decodeValue(spec.key, key, transport), decodeValue(spec.item, item, transport)]));
        case "record":
        case "flags":
            if (!(value instanceof Map)) throw new TypeError("记录需要 CBOR 映射");
            return Object.fromEntries(spec.fields.map((field: Spec) => [field.typescript, decodeValue(field.type, value.get(field.name), transport)]));
        case "variant": {
            if (!(value instanceof Map)) throw new TypeError("variant 需要 CBOR 映射");
            const entries = Array.from(value);
            if (entries.length !== 1) throw new TypeError("variant 需要单键映射");
            const [name, payload] = entries[0];
            const entry = spec.cases.find((entry: Spec) => entry.name === name);
            if (!entry) throw new TypeError("variant 分支错误");
            if (entry.type === null) {
                scalar("unit", payload);
                return { kind: name };
            }
            return { kind: name, value: decodeValue(entry.type, payload, transport) };
        }
        case "enum":
            if (!spec.cases.includes(value)) throw new TypeError("enum 值错误");
            return value;
        case "result": {
            if (!(value instanceof Map)) throw new TypeError("result 需要 CBOR 映射");
            const entries = Array.from(value);
            if (entries.length !== 1 || !["ok", "err"].includes(entries[0][0])) throw new TypeError("result 需要 ok/err 单键映射");
            const [branch, payload] = entries[0];
            return { [branch]: decodeValue(spec[branch], payload, transport) };
        }
        case "resource":
        case "error-context":
            if (!(value instanceof Tagged) || value.tag !== 60000) throw new TypeError("服务值需要对象引用标签");
            return new CLIENTS[spec.class ?? "ErrorContext"](transport.importResource(value, spec.service ?? "cordis.error-context@1.0", spec.borrowed ?? false));
        default:
            return scalar(spec.kind, value);
    }
}

async function invoke(transport: Transport, service: string, index: number, args: unknown[]): Promise<unknown> {
    const descriptor = SCHEMA.services.find((entry) => entry.interface === service)!;
    const method = descriptor.methods[index];
    const values = args.map((value, index) => encodeValue(method.params[index].type, value, transport, true));
    const result = await transport.invoke(service, method.name, values, method.async);
    return decodeValue(method.result, result, transport, true);
}

export async function dispatch(provider: object, transport: Transport, service: string, name: string, args: unknown[]): Promise<unknown> {
    const descriptor = SCHEMA.services.find((entry) => entry.interface === service);
    const method = descriptor?.methods.find((entry) => entry.name === name);
    if (!method || args.length !== method.params.length) throw new TypeError("方法或参数数量错误");
    const values = args.map((value, index) => decodeValue(method.params[index].type, value, transport, true));
    const callback = (provider as Record<string, (...values: unknown[]) => unknown>)[method.typescript];
    let result = callback.apply(provider, values);
    if (method.async) result = await result;
    return encodeValue(method.result, result, transport, true);
}


export const SCHEMA: Schema = {
  "package": "cordis:kv@1.0.0",
  "services": [
    {
      "class": "Kv",
      "interface": "kv@1.0",
      "methods": [
        {
          "async": false,
          "error": {
            "kind": "string"
          },
          "name": "get",
          "params": [
            {
              "name": "key",
              "type": {
                "kind": "string"
              }
            }
          ],
          "python": "get",
          "result": {
            "ref": "4"
          },
          "since": null,
          "typescript": "get"
        },
        {
          "async": false,
          "error": {
            "kind": "string"
          },
          "name": "put",
          "params": [
            {
              "name": "key",
              "type": {
                "kind": "string"
              }
            },
            {
              "name": "value",
              "type": {
                "ref": "0"
              }
            }
          ],
          "python": "put",
          "result": {
            "kind": "unit"
          },
          "since": null,
          "typescript": "put"
        },
        {
          "async": false,
          "error": {
            "kind": "string"
          },
          "name": "keys",
          "params": [],
          "python": "keys",
          "result": {
            "ref": "7"
          },
          "since": null,
          "typescript": "keys"
        },
        {
          "async": true,
          "error": {
            "kind": "string"
          },
          "name": "flush",
          "params": [],
          "python": "flush",
          "result": {
            "kind": "unit"
          },
          "since": null,
          "typescript": "flush"
        }
      ]
    }
  ],
  "types": {
    "0": {
      "item": {
        "kind": "u8"
      },
      "kind": "list"
    },
    "1": {
      "item": {
        "ref": "0"
      },
      "key": {
        "kind": "string"
      },
      "kind": "map"
    },
    "2": {
      "class": "Snapshot",
      "fields": [
        {
          "name": "entries",
          "python": "entries",
          "type": {
            "ref": "1"
          },
          "typescript": "entries"
        }
      ],
      "kind": "record"
    },
    "3": {
      "class": "Changed",
      "fields": [
        {
          "name": "key",
          "python": "key",
          "type": {
            "kind": "string"
          },
          "typescript": "key"
        }
      ],
      "kind": "record"
    },
    "4": {
      "item": {
        "ref": "0"
      },
      "kind": "option"
    },
    "5": {
      "err": {
        "kind": "string"
      },
      "kind": "result",
      "ok": {
        "ref": "4"
      }
    },
    "6": {
      "err": {
        "kind": "string"
      },
      "kind": "result",
      "ok": {
        "kind": "unit"
      }
    },
    "7": {
      "item": {
        "kind": "string"
      },
      "kind": "list"
    },
    "8": {
      "err": {
        "kind": "string"
      },
      "kind": "result",
      "ok": {
        "ref": "7"
      }
    }
  }
};

export interface Kv {
    get(key: string): Option<Uint8Array>;
    put(key: string, value: Uint8Array): void;
    keys(): Array<string>;
    flush(): Promise<void>;
}
export class KvClient {
    constructor(private readonly transport: Transport) {}
    async get(key: string): Promise<Option<Uint8Array>> {
        return await invoke(this.transport, "kv@1.0", 0, [key]) as Option<Uint8Array>;
    }
    async put(key: string, value: Uint8Array): Promise<void> {
        return await invoke(this.transport, "kv@1.0", 1, [key, value]) as void;
    }
    async keys(): Promise<Array<string>> {
        return await invoke(this.transport, "kv@1.0", 2, []) as Array<string>;
    }
    async flush(): Promise<void> {
        return await invoke(this.transport, "kv@1.0", 3, []) as void;
    }
}

export interface Snapshot {
    entries: Map<string, Array<number>>;
}

export interface Changed {
    key: string;
}


const CLIENTS: Record<string, new (transport: Transport) => unknown> = {Kv: KvClient};
