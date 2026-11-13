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
            if (resolved(spec.item).kind === "u8" && spec.length === undefined) {
                if (!(value instanceof Uint8Array)) throw new TypeError("字节值需要 Uint8Array");
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
            if (resolved(spec.item).kind === "u8" && spec.length === undefined) {
                if (value instanceof Uint8Array) return value;
                if (Array.isArray(value)) return new Uint8Array(value.map(item => scalar("u8", item) as number));
                throw new TypeError("字节值需要字节串或 u8 数组");
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
            if ((spec.kind === "u64" || spec.kind === "s64") && typeof value === "number" && Number.isSafeInteger(value)) {
                value = BigInt(value);
            }
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
    result = await result;
    return encodeValue(method.result, result, transport, true);
}


export const SCHEMA: Schema = {
  "package": "cordis:http@1.0.0",
  "services": [
    {
      "class": "Streams",
      "interface": "streams@1.0",
      "methods": []
    },
    {
      "class": "CordisByteStreamStreamsByteStream",
      "interface": "cordis.byte-stream@1.0",
      "methods": [
        {
          "async": true,
          "error": {
            "kind": "string"
          },
          "name": "read",
          "params": [
            {
              "name": "max-bytes",
              "type": {
                "kind": "u32"
              }
            }
          ],
          "python": "read",
          "result": {
            "ref": "3"
          },
          "since": null,
          "typescript": "read"
        }
      ]
    },
    {
      "class": "Http",
      "interface": "cordis.http@1.0",
      "methods": [
        {
          "async": true,
          "error": {
            "kind": "string"
          },
          "name": "route",
          "params": [
            {
              "name": "spec",
              "type": {
                "ref": "12"
              }
            },
            {
              "name": "handler",
              "type": {
                "ref": "25"
              }
            }
          ],
          "python": "route",
          "result": {
            "kind": "u64"
          },
          "since": null,
          "typescript": "route"
        },
        {
          "async": true,
          "error": {
            "kind": "string"
          },
          "name": "unroute",
          "params": [
            {
              "name": "id",
              "type": {
                "kind": "u64"
              }
            }
          ],
          "python": "unroute",
          "result": {
            "kind": "unit"
          },
          "since": null,
          "typescript": "unroute"
        }
      ]
    },
    {
      "class": "HttpHandler",
      "interface": "cordis.http-handler@1.0",
      "methods": [
        {
          "async": true,
          "error": {
            "kind": "string"
          },
          "name": "handle",
          "params": [
            {
              "name": "request",
              "type": {
                "ref": "15"
              }
            }
          ],
          "python": "handle",
          "result": {
            "ref": "16"
          },
          "since": null,
          "typescript": "handle"
        }
      ]
    },
    {
      "class": "Pages",
      "interface": "cordis.http-page@1.0",
      "methods": [
        {
          "async": true,
          "error": {
            "kind": "string"
          },
          "name": "attributes",
          "params": [
            {
              "name": "key",
              "type": {
                "ref": "9"
              }
            }
          ],
          "python": "attributes",
          "result": {
            "ref": "23"
          },
          "since": null,
          "typescript": "attributes"
        }
      ]
    }
  ],
  "types": {
    "0": {
      "borrowed": false,
      "class": "CordisByteStreamStreamsByteStream",
      "kind": "resource",
      "service": "cordis.byte-stream@1.0"
    },
    "1": {
      "borrowed": true,
      "class": "CordisByteStreamStreamsByteStream",
      "kind": "resource",
      "service": "cordis.byte-stream@1.0"
    },
    "10": {
      "cases": [
        {
          "class": "BodyEmpty",
          "name": "empty",
          "type": null
        },
        {
          "class": "BodyInline",
          "name": "inline",
          "type": {
            "ref": "9"
          }
        },
        {
          "class": "BodyStream",
          "name": "stream",
          "type": {
            "ref": "8"
          }
        }
      ],
      "class": "Body",
      "kind": "variant"
    },
    "11": {
      "item": {
        "kind": "string"
      },
      "kind": "list"
    },
    "12": {
      "class": "RouteSpec",
      "fields": [
        {
          "name": "prefix",
          "python": "prefix",
          "type": {
            "kind": "string"
          },
          "typescript": "prefix"
        },
        {
          "name": "methods",
          "python": "methods",
          "type": {
            "ref": "11"
          },
          "typescript": "methods"
        },
        {
          "name": "max-body-bytes",
          "python": "max_body_bytes",
          "type": {
            "ref": "6"
          },
          "typescript": "maxBodyBytes"
        },
        {
          "name": "timeout-ms",
          "python": "timeout_ms",
          "type": {
            "ref": "6"
          },
          "typescript": "timeoutMs"
        }
      ],
      "kind": "record"
    },
    "13": {
      "items": [
        {
          "kind": "string"
        },
        {
          "kind": "string"
        }
      ],
      "kind": "tuple"
    },
    "14": {
      "item": {
        "ref": "13"
      },
      "kind": "list"
    },
    "15": {
      "class": "HttpRequest",
      "fields": [
        {
          "name": "method",
          "python": "method",
          "type": {
            "kind": "string"
          },
          "typescript": "method"
        },
        {
          "name": "path",
          "python": "path",
          "type": {
            "kind": "string"
          },
          "typescript": "path"
        },
        {
          "name": "prefix",
          "python": "prefix",
          "type": {
            "kind": "string"
          },
          "typescript": "prefix"
        },
        {
          "name": "query",
          "python": "query",
          "type": {
            "kind": "string"
          },
          "typescript": "query"
        },
        {
          "name": "headers",
          "python": "headers",
          "type": {
            "ref": "14"
          },
          "typescript": "headers"
        },
        {
          "name": "peer",
          "python": "peer",
          "type": {
            "kind": "string"
          },
          "typescript": "peer"
        },
        {
          "name": "body",
          "python": "body",
          "type": {
            "ref": "10"
          },
          "typescript": "body"
        }
      ],
      "kind": "record"
    },
    "16": {
      "class": "HttpResponse",
      "fields": [
        {
          "name": "status",
          "python": "status",
          "type": {
            "kind": "u16"
          },
          "typescript": "status"
        },
        {
          "name": "headers",
          "python": "headers",
          "type": {
            "ref": "14"
          },
          "typescript": "headers"
        },
        {
          "name": "body",
          "python": "body",
          "type": {
            "ref": "10"
          },
          "typescript": "body"
        }
      ],
      "kind": "record"
    },
    "17": {
      "borrowed": false,
      "class": "HttpHandler",
      "kind": "resource",
      "service": "cordis.http-handler@1.0"
    },
    "18": {
      "borrowed": true,
      "class": "HttpHandler",
      "kind": "resource",
      "service": "cordis.http-handler@1.0"
    },
    "19": {
      "err": {
        "kind": "string"
      },
      "kind": "result",
      "ok": {
        "ref": "16"
      }
    },
    "2": {
      "item": {
        "kind": "u8"
      },
      "kind": "list"
    },
    "20": {
      "err": {
        "kind": "string"
      },
      "kind": "result",
      "ok": {
        "kind": "u64"
      }
    },
    "21": {
      "err": {
        "kind": "string"
      },
      "kind": "result",
      "ok": {
        "kind": "unit"
      }
    },
    "22": {
      "class": "PageAttributes",
      "fields": [
        {
          "name": "headers",
          "python": "headers",
          "type": {
            "ref": "14"
          },
          "typescript": "headers"
        },
        {
          "name": "peer",
          "python": "peer",
          "type": {
            "kind": "string"
          },
          "typescript": "peer"
        }
      ],
      "kind": "record"
    },
    "23": {
      "item": {
        "ref": "22"
      },
      "kind": "option"
    },
    "24": {
      "err": {
        "kind": "string"
      },
      "kind": "result",
      "ok": {
        "ref": "23"
      }
    },
    "25": {
      "borrowed": false,
      "class": "HttpHandler",
      "kind": "resource",
      "service": "cordis.http-handler@1.0"
    },
    "3": {
      "item": {
        "ref": "2"
      },
      "kind": "option"
    },
    "4": {
      "err": {
        "kind": "string"
      },
      "kind": "result",
      "ok": {
        "ref": "3"
      }
    },
    "5": {
      "class": "CordisHttpHttpByteStream",
      "item": {
        "ref": "0"
      },
      "kind": "alias"
    },
    "6": {
      "item": {
        "kind": "u64"
      },
      "kind": "option"
    },
    "7": {
      "borrowed": false,
      "class": "CordisByteStreamStreamsByteStream",
      "kind": "resource",
      "service": "cordis.byte-stream@1.0"
    },
    "8": {
      "class": "StreamBody",
      "fields": [
        {
          "name": "length",
          "python": "length",
          "type": {
            "ref": "6"
          },
          "typescript": "length"
        },
        {
          "name": "source",
          "python": "source",
          "type": {
            "ref": "7"
          },
          "typescript": "source"
        }
      ],
      "kind": "record"
    },
    "9": {
      "item": {
        "kind": "u8"
      },
      "kind": "list"
    }
  }
};

export interface Streams {
}
export class StreamsClient {
    constructor(private readonly transport: Transport) {}
}

export interface CordisByteStreamStreamsByteStream {
    read(maxBytes: number): Promise<Option<Uint8Array>>;
}
export class CordisByteStreamStreamsByteStreamClient {
    constructor(private readonly transport: Transport) {}
    async read(maxBytes: number): Promise<Option<Uint8Array>> {
        return await invoke(this.transport, "cordis.byte-stream@1.0", 0, [maxBytes]) as Option<Uint8Array>;
    }
}

export interface Http {
    route(spec: RouteSpec, handler: HttpHandler): Promise<bigint>;
    unroute(id: bigint): Promise<void>;
}
export class HttpClient {
    constructor(private readonly transport: Transport) {}
    async route(spec: RouteSpec, handler: HttpHandler): Promise<bigint> {
        return await invoke(this.transport, "cordis.http@1.0", 0, [spec, handler]) as bigint;
    }
    async unroute(id: bigint): Promise<void> {
        return await invoke(this.transport, "cordis.http@1.0", 1, [id]) as void;
    }
}

export interface HttpHandler {
    handle(request: HttpRequest): Promise<HttpResponse>;
}
export class HttpHandlerClient {
    constructor(private readonly transport: Transport) {}
    async handle(request: HttpRequest): Promise<HttpResponse> {
        return await invoke(this.transport, "cordis.http-handler@1.0", 0, [request]) as HttpResponse;
    }
}

export interface Pages {
    attributes(key: Uint8Array): Promise<Option<PageAttributes>>;
}
export class PagesClient {
    constructor(private readonly transport: Transport) {}
    async attributes(key: Uint8Array): Promise<Option<PageAttributes>> {
        return await invoke(this.transport, "cordis.http-page@1.0", 0, [key]) as Option<PageAttributes>;
    }
}

export type CordisHttpHttpByteStream = CordisByteStreamStreamsByteStream;
export interface StreamBody {
    length: Option<bigint>;
    source: CordisByteStreamStreamsByteStream;
}

export type Body = { kind: "empty" } | { kind: "inline"; value: Uint8Array } | { kind: "stream"; value: StreamBody };
export interface RouteSpec {
    prefix: string;
    methods: Array<string>;
    maxBodyBytes: Option<bigint>;
    timeoutMs: Option<bigint>;
}

export interface HttpRequest {
    method: string;
    path: string;
    prefix: string;
    query: string;
    headers: Array<[string, string]>;
    peer: string;
    body: Body;
}

export interface HttpResponse {
    status: number;
    headers: Array<[string, string]>;
    body: Body;
}

export interface PageAttributes {
    headers: Array<[string, string]>;
    peer: string;
}


const CLIENTS: Record<string, new (transport: Transport) => unknown> = {Streams: StreamsClient, CordisByteStreamStreamsByteStream: CordisByteStreamStreamsByteStreamClient, Http: HttpClient, HttpHandler: HttpHandlerClient, Pages: PagesClient};
