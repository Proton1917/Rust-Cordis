/** DOC-16 §4：协议错误保留代码、信息和业务详情。 */
export class CordisError extends Error {
    constructor(
        readonly code: number,
        message: string,
        readonly detail: unknown = null,
    ) {
        super(message);
        this.name = "CordisError";
    }
    wire(): object {
        return { code: this.code, message: this.message, detail: this.detail };
    }
}

export type Identifier = bigint;
export type Fields = Record<string, any>;
export type MaybePromise<T> = T | Promise<T>;

/** DOC-16 §4：协议记录与应用 Map 分别转换，应用值保留 CBOR 类型。 */
export function fields(value: unknown): Fields {
    if (!(value instanceof Map)) throw new CordisError(2, "协议记录需要 CBOR 映射");
    return Object.fromEntries(value);
}

/** DOC-16 §5：编号使用完整 u64 表示，接收小整数时归一为 bigint。 */
export function identifier(value: unknown): Identifier {
    if ((typeof value !== "number" || !Number.isSafeInteger(value)) && typeof value !== "bigint") {
        throw new CordisError(2, "编号需要无符号整数");
    }
    const result = BigInt(value);
    if (result <= 0n || result > 0xffffffffffffffffn) throw new CordisError(2, "编号超出 u64 范围");
    return result;
}

/** DOC-16 §7：配置以 JavaScript 属性读取，字节串与标签保持自身类型。 */
export function configuration(value: any): any {
    if (value instanceof Map)
        return Object.fromEntries(Array.from(value, ([key, item]) => [key, configuration(item)]));
    if (Array.isArray(value)) return value.map(configuration);
    return value;
}

export interface Transport {
    invoke(
        service: string,
        method: string,
        args: unknown[],
        asynchronous: boolean,
    ): Promise<unknown>;
    exportResource(
        value: unknown,
        service: string,
        borrowed: boolean,
    ): { tag: number; value: unknown };
    importResource(
        value: { tag: number; value: unknown },
        service: string,
        borrowed: boolean,
    ): Transport;
}

export interface Binding {
    Tagged: new (tag: number, value: unknown) => { tag: number; value: unknown };
    SCHEMA: {
        services: Array<{ class: string; interface: string; methods: Array<{ name: string }> }>;
    };
    dispatch(
        provider: object,
        transport: Transport,
        service: string,
        name: string,
        args: unknown[],
    ): Promise<unknown>;
}
