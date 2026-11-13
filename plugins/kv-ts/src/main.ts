/** DOC-15 M19、AU-18：共享 kv.snapshot 状态和按元数据绑定的键空间。 */
import {Context, CordisError, Frozen, run} from "cordis-sdk";
import * as binding from "./bindings.js";

class Store {
    constructor(readonly context: Context, readonly values: Map<string, Uint8Array>, readonly frozen: Frozen, readonly namespace = "") {}
    scope(metadata: Map<string, unknown>): Store {
        return new Store(this.context, this.values, this.frozen, String(metadata.get("namespace") ?? ""));
    }
    key(key: string): string { return this.namespace ? `${this.namespace}/${key}` : key; }
    get(key: string): Uint8Array | null { return this.values.get(this.key(key)) ?? null; }
    async put(key: string, value: Uint8Array): Promise<void> {
        this.frozen.check();
        this.values.set(this.key(key), value);
        await this.context.emit("kv.changed@1.0", {key});
    }
    keys(): string[] {
        const prefix = this.namespace ? `${this.namespace}/` : "";
        return [...this.values.keys()].filter(key => key.startsWith(prefix)).map(key => key.slice(prefix.length)).sort();
    }
    async flush(): Promise<void> {}
}

/** DOC-11 §5.3、ADR-0039：继承兼容数组与字节串，导出统一使用字节串。 */
async function apply(context: Context): Promise<void> {
    const state = context.inherited as Map<string, any> | null;
    if (state && (state.get("schema") !== "kv.snapshot" || state.get("version") !== 1)) {
        throw new CordisError(2, "kv-store 需要 kv.snapshot 版本 1");
    }
    const entries: Map<string, number[]> = state?.get("data").get("entries") ?? new Map();
    const values = new Map([...entries].map(([key, value]) => [key, new Uint8Array(value)]));
    const frozen = new Frozen();
    await context.exportState("kv.snapshot", 1, () => ({
        entries: new Map(values),
    }), frozen);
    await context.provide("kv@1.0", new Store(context, values, frozen), binding, true);
}

await run("kv", [{name: "kv-store", apply, provides: ["kv@1.0"],
    stateSchema: "kv.snapshot", stateVersionMin: 1, stateVersionMax: 1}]);
