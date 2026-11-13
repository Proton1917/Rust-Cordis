/** DOC-16 §3：编码和解码边界检查完整值图的深度、集合和节点预算。 */
import { Tag } from "cbor-x";
import { CordisError } from "./types.js";

export interface ValueLimits {
    maxDepth: number;
    maxCollection: number;
    maxValues: number;
}

export function checkValue(root: unknown, limits: ValueLimits): void {
    const pending: Array<[unknown, number]> = [[root, 0]];
    let nodes = 0;
    while (pending.length) {
        const [value, depth] = pending.pop()!;
        if (++nodes > limits.maxValues || depth > limits.maxDepth)
            throw new CordisError(8, "值图超过深度或节点预算");
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
