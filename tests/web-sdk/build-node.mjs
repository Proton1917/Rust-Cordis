import {createRequire} from "node:module";
import {resolve} from "node:path";

const [modules, source, destination] = process.argv.slice(2);
const workspace = resolve(modules, "../..");
const require = createRequire(resolve(workspace, "sdks/typescript/package.json"));
const {build} = require("esbuild");
await build({
    entryPoints: [resolve(source, "main.ts")],
    outfile: resolve(destination, "main.cjs"),
    bundle: true,
    format: "cjs",
    platform: "node",
    target: "node22",
    alias: {"cordis-sdk": resolve(workspace, "sdks/typescript/cordis-sdk/src/index.ts")},
    sourcemap: true,
});
