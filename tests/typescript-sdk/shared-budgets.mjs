import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {test} from "node:test";
import {Runtime} from "../../sdks/typescript/cordis-sdk/dist/runtime.js";
import {checkMessage} from "../../sdks/typescript/cordis-sdk/dist/budget.js";
const runtime = new Runtime("vectors", "1", [], {destroy() {}}, {write() {}});
for (const line of readFileSync(new URL("../up1/value-budgets.tsv", import.meta.url), "utf8").trim().split("\n")) {
    if (line.startsWith("#")) continue;
    const [name, depth, collection, values, frame, accepted, hex] = line.split("\t");
    test(`shared budget: ${name}`, () => {
        const bytes = Buffer.from(hex, "hex");
        const check = () => {
            assert.ok(bytes.length <= Number(frame));
            checkMessage(runtime.connection.decoder.decode(bytes), {
                maxDepth: Number(depth), maxCollection: Number(collection), maxValues: Number(values),
            });
        };
        if (accepted === "1") assert.doesNotThrow(check);
        else assert.throws(check);
    });
}
