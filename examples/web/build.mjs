import {build} from "esbuild";
import {fileURLToPath} from "node:url";
import {dirname, join} from "node:path";

const root = dirname(fileURLToPath(import.meta.url));
for (const name of ["web-notes", "kv-dashboard-rust", "kv-dashboard-python"]) {
    await build({
        entryPoints: [join(root, "source/plugins", name, "ui/main.js")],
        outfile: join(root, "source/plugins", name, "web/index.js"),
        bundle: true, format: "esm", platform: "browser", target: "es2022",
        external: ["/_cordis/cordis-web.js"], sourcemap: true,
    });
}
