import {Context, run} from "cordis-sdk";
import * as binding from "./bytes-binding.js";

class Provider implements binding.Exchange {
    async exchange(value: binding.Bundle): Promise<binding.Bundle> { return value; }
}

async function apply(context: Context): Promise<void> {
    await context.provide("test.byte-payload@1.0", new Provider(), binding);
}

await run("byte-provider-ts", [{name: "bytes-provider", apply, provides: ["test.byte-payload@1.0"]}]);
