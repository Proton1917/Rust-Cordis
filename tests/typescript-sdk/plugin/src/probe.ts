/** DOC-15 M19、AU-18：真实核心中的跨插口字节调用、拦截视图和清理。 */
import assert from "node:assert/strict";
import {writeFile} from "node:fs/promises";
import {Context, close, run} from "cordis-sdk";
import * as binding from "./bindings.js";

async function apply(context: Context, config: {scoped: boolean; result: string; cleanup: string}): Promise<void> {
    const store = await context.get("kv@1.0", binding, binding.KvClient);
    try {
        assert.equal(await store.get("missing"), null);
        await store.put("roundtrip", new Uint8Array([0, 127, 255]));
        assert.deepEqual(Array.from((await store.get("roundtrip")) as Uint8Array), [0, 127, 255]);
        assert((await store.keys()).includes("roundtrip"));
        await store.flush();
        if (config.scoped) {
            const scoped = await context.intercept("kv@1.0", {namespace: "tenant"});
            const view = await scoped.get("kv@1.0", binding, binding.KvClient);
            try {
                await view.put("roundtrip", new Uint8Array([42]));
                assert.deepEqual(Array.from((await view.get("roundtrip")) as Uint8Array), [42]);
                assert.deepEqual(Array.from((await store.get("roundtrip")) as Uint8Array), [0, 127, 255]);
            } finally { close(view); scoped.close(); }
        }
    } finally { close(store); }
    const effect = await context.effect(async scope => {
        await scope.defer(async () => { await writeFile(config.cleanup, "cleaned\n"); });
    }, "probe", true);
    await effect.settled();
    effect.close();
    assert(await context.isCurrent());
    await writeFile(config.result, JSON.stringify({bytes: [0, 127, 255], scoped: config.scoped}));
}

await run("typescript-probe", [{name: "probe", apply, inject: {"kv@1.0": true}}]);
