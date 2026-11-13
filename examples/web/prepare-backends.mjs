import {cp, copyFile} from "node:fs/promises";
import {basename, dirname, join, resolve, sep} from "node:path";
import {fileURLToPath} from "node:url";
import {parseArgs} from "node:util";

const {values} = parseArgs({options: {workspace: {type: "string"}, native: {type: "string"}, http: {type: "string"}}});
if (!values.workspace || !values.native) throw new Error("需要 --workspace 和 --native 参数");
const root = dirname(fileURLToPath(import.meta.url));
const workspace = resolve(values.workspace);
await copyFile(values.native, join(root, "source/plugins/kv-dashboard-rust", basename(values.native)));
if (values.http) await copyFile(values.http, join(root, "source/plugins", basename(values.http)));
const python = join(root, "source/plugins/kv-dashboard-python");
const options = {recursive: true, filter: source => !source.split(sep).includes("__pycache__")};
await cp(join(workspace, "plugins/kv-py/kv_py"), join(python, "kv_py"), options);
await cp(join(workspace, "sdks/python/cordis_sdk"), join(python, "cordis_sdk"), options);
