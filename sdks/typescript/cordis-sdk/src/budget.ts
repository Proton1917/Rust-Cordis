/** DOC-16 §3：编码和解码边界检查完整值图的深度、集合和节点预算。 */
import { Tag } from "cbor-x";
import { CordisError } from "./types.js";

export interface ValueLimits {
    maxDepth: number;
    maxCollection: number;
    maxValues: number;
}

export function checkValue(root: unknown, limits: ValueLimits, overhead = 0): void {
    const pending: Array<[unknown, number]> = [[root, overhead]];
    let nodes = 0;
    while (pending.length) {
        const [value, depth] = pending.pop()!;
        if (++nodes > limits.maxValues)
            throw new CordisError(8, "值图超过深度或节点预算");
        const container = value instanceof Tag || Array.isArray(value) || value instanceof Map ||
            (value && typeof value === "object" && !(value instanceof Uint8Array));
        if (container && depth >= limits.maxDepth)
            throw new CordisError(8, "值图超过深度预算");
        if (value instanceof Tag) pending.push([value.value, depth + 1]);
        else if (Array.isArray(value)) {
            if (value.length > limits.maxCollection) throw new CordisError(8, "数组超过元素预算");
            for (const item of value) pending.push([item, depth + 1]);
        } else if (value instanceof Map) {
            if (value.size > limits.maxCollection) throw new CordisError(8, "映射超过元素预算");
            for (const [key, item] of value) pending.push([key, depth + 1], [item, depth + 1]);
        } else if (value && typeof value === "object" && !(value instanceof Uint8Array)) {
            const entries = Object.entries(value);
            if (entries.length > limits.maxCollection) throw new CordisError(8, "记录超过字段预算");
            for (const [key, item] of entries) pending.push([key, depth + 1], [item, depth + 1]);
        }
    }
}

/** DOC-16 §3：集合与节点计数属于参数/结果/诊断值，包络仅计入深度。 */
export function checkMessage(message: unknown, limits: ValueLimits): void {
    // Unknown envelope fields still consume parser nesting, matching Rust's
    // recursion limit. Their collections/nodes retain the envelope's own schema.
    const children = (value: unknown): Iterable<unknown> | undefined => {
        if (value instanceof Tag) return [value.value];
        if (Array.isArray(value)) return value;
        if (value instanceof Map) return (function* () {
            for (const [key, item] of value) { yield key; yield item; }
        })();
        if (value && typeof value === "object" && !(value instanceof Uint8Array))
            return (function* () {
                for (const key in value) if (Object.hasOwn(value, key)) {
                    yield key; yield (value as Record<string, unknown>)[key];
                }
            })();
        return undefined;
    };
    const stack: Array<[Iterator<unknown>, number]> = [[([message] as unknown[]).values(), 0]];
    while (stack.length) {
        const [iterator, depth] = stack[stack.length - 1];
        const next = iterator.next();
        if (next.done) { stack.pop(); continue; }
        const nested = children(next.value);
        if (nested) {
            if (depth >= limits.maxDepth) throw new CordisError(8, "消息超过深度预算");
            stack.push([nested[Symbol.iterator](), depth + 1]);
        }
    }
    const get = (value: unknown, key: string): unknown => value instanceof Map
        ? value.get(key)
        : value && typeof value === "object" ? (value as Record<string, unknown>)[key] : undefined;
    const has = (value: unknown, key: string): boolean => value instanceof Map
        ? value.has(key)
        : !!value && typeof value === "object" && Object.hasOwn(value, key);
    for (const kind of ["request", "notification"]) {
        const body = get(message, kind);
        if (has(body, "params")) checkValue(get(body, "params"), limits, 2);
    }
    const result = get(get(message, "response"), "result");
    if (has(result, "ok")) checkValue(get(result, "ok"), limits, 3);
    const error = get(result, "error");
    if (has(error, "detail")) checkValue(get(error, "detail"), limits, 4);
}
