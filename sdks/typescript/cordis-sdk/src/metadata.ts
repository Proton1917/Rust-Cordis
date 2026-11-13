/** ADR-0046 §3.11：调用身份描述直接调用方，授权由提供方决定。 */
import {CordisError} from "./types.js";

export type Caller = {instance: string; page_of?: never} | {page_of: string; instance?: never};

export function caller(metadata: Map<string, unknown>): Caller | null {
    const value = metadata.get("cordis.caller");
    if (value === undefined) return null;
    if (!(value instanceof Map) || value.size !== 1) throw new CordisError(2, "调用身份需要 instance 或 page_of");
    if (typeof value.get("instance") === "string") return {instance: value.get("instance")};
    if (typeof value.get("page_of") === "string") return {page_of: value.get("page_of")};
    throw new CordisError(2, "调用身份名称需要字符串");
}
