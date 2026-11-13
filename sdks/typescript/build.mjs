import {readFile, mkdir, writeFile, rename} from "node:fs/promises";
import {existsSync} from "node:fs";
import {dirname, resolve} from "node:path";
import {fileURLToPath} from "node:url";
import {build} from "esbuild";
import {compile} from "./cordis-sdk/build.mjs";

const root = dirname(fileURLToPath(import.meta.url));

async function licenses(inputs) {
    const packages = new Map();
    for (const input of Object.keys(inputs)) {
        if (!input.includes("node_modules/")) continue;
        let directory = dirname(resolve(root, input));
        while (!existsSync(resolve(directory, "package.json"))) {
            const parent = dirname(directory);
            if (parent === directory) throw new Error(`依赖缺少package.json：${input}`);
            directory = parent;
        }
        const metadata = JSON.parse(await readFile(resolve(directory, "package.json"), "utf8"));
        if (packages.has(metadata.name)) continue;
        const license = ["LICENSE", "LICENSE.md", "LICENSE.txt"].map(name => resolve(directory, name)).find(existsSync);
        if (!license) throw new Error(`依赖缺少许可证：${metadata.name}`);
        packages.set(metadata.name, `${metadata.name} ${metadata.version}\n${await readFile(license, "utf8")}`);
    }
    return [...packages.entries()].sort(([left], [right]) => left.localeCompare(right)).map(([, text]) => text).join("\n\n");
}

compile(resolve(root, "cordis-sdk"));
compile(resolve(root, "cordis-web"));
const result = await build({absWorkingDir: root, entryPoints: ["cordis-web/src/index.ts"], bundle: true,
    format: "esm", platform: "browser", target: "es2022", write: false, metafile: true});
const notices = await licenses(result.metafile.inputs);
const code = `/*!\n${notices}\n*/\n${result.outputFiles[0].text}`;
for (const path of [resolve(root, "cordis-web/dist/cordis-web.js"), resolve(root, "../../plugins/http/assets/cordis-web.js")]) {
    await mkdir(dirname(path), {recursive: true});
    await writeFile(path + ".next", code);
    await rename(path + ".next", path);
    console.log(`已生成 ${path}`);
}
