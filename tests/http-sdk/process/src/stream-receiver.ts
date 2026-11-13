import {Context, run, streamToFile} from "cordis-sdk";
import {writeFileSync} from "node:fs";
import * as binding from "./stream-binding.js";

async function apply(context: Context, config: Record<string, string>): Promise<void> {
    const files = await context.get("test.stream-files@1.0", binding, binding.FilesClient);
    const source = await files.open();
    const count = await streamToFile(source, config.output);
    writeFileSync(config.result, JSON.stringify({bytes: count, chunk_limit: 1048576, frame_limit: 4198400}));
}

await run("stream-receiver-ts", [{name: "stream-probe", apply, inject: {"test.stream-files@1.0": true}}], "0.1.0", {maxFrame: 4198400});
