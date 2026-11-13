import assert from "node:assert/strict";
import {writeFile} from "node:fs/promises";
import {Context, CordisError, run} from "cordis-sdk";
import * as bindings from "./bindings.js";

async function exercise(context: Context, config: {result: string}): Promise<void> {
    const remote = await context.getDynamic("kv@1.0", bindings);
    await context.defer(() => remote.close());
    await remote.invoke("kv@1.0", "put", ["typescript-dynamic", new Uint8Array([0, 255])]);
    const value = await remote.invoke("kv@1.0", "get", ["typescript-dynamic"]);
    assert(value instanceof Uint8Array);
    assert.deepEqual(Array.from(value), [0, 255]);
    const result: Record<string, unknown> = {value: Array.from(value)};
    for (const [label, method, args, expected] of [
        ["unknown_method", "absent-method", [], 3],
        ["forged_resource", "put", ["forged", new bindings.Tagged(60000, 1)], 2],
    ] as const) {
        try {
            await remote.invoke("kv@1.0", method, [...args]);
            assert.fail(label + " 调用取得成功");
        } catch (error) {
            assert(error instanceof CordisError);
            assert.equal(error.code, expected);
            result[label] = error.code;
        }
    }
    await writeFile(config.result, JSON.stringify(result));
}

async function apply(context: Context, config: {result: string}): Promise<void> {
    const effect = await context.effect(scope => exercise(scope, config), "dynamic-probe", true);
    await effect.settled();
}

await run("dynamic-script-tests", [{name: "probe", inject: {"kv@1.0": true}, apply}]);
