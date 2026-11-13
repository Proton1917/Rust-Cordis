import {createRequire} from "node:module";
import {resolve} from "node:path";

const [modules, source, destination] = process.argv.slice(2);
const require = createRequire(resolve(modules, "../../sdks/typescript/package.json"));
const {build} = require("esbuild");
await build({
    entryPoints: [resolve(source, "main.js")],
    outfile: resolve(destination, "web/index.js"),
    bundle: true,
    format: "esm",
    platform: "browser",
    target: "es2022",
    external: ["/_cordis/cordis-web.js"],
    sourcemap: true,
});
