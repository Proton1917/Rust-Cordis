/** ADR-0046：TypeScript 处理方与 Rust、Python 共用 WIT 契约和实例数据目录。 */
import {mkdirSync, statSync, unlinkSync, writeFileSync} from "node:fs";
import {join} from "node:path";
import {Context, CordisError, Remote, close, copyStream, fromFile, registerRoute, run, streamToFile} from "cordis-sdk";
import * as api from "./bindings.js";

function response(value: unknown): api.HttpResponse {
    return {status: 200, headers: [["content-type", "application/json"]], body: {kind: "inline", value: new TextEncoder().encode(JSON.stringify(value))}};
}

class Handler implements api.HttpHandler {
    private readonly file: string;
    constructor(directory: string, private readonly label: string) {
        mkdirSync(directory, {recursive: true});
        this.file = join(directory, "upload.bin");
    }
    async handle(request: api.HttpRequest): Promise<api.HttpResponse> {
        const path = request.path.slice(request.prefix.length);
        if (path === "/file") {
            if (["PUT", "POST"].includes(request.method)) {
                let size: number;
                if (request.body.kind === "stream") size = await streamToFile(request.body.value.source, this.file);
                else {
                    const value = new Uint8Array(request.body.kind === "inline" ? request.body.value : []);
                    writeFileSync(this.file, value);
                    size = value.byteLength;
                }
                return response({stored: size, label: this.label});
            }
            if (request.method === "GET") return {status: 200, headers: [["content-type", "application/octet-stream"]],
                body: {kind: "stream", value: {length: BigInt(statSync(this.file).size), source: fromFile(this.file)}}};
            if (request.method === "DELETE") { unlinkSync(this.file); return response({removed: true}); }
        }
        if (path === "/error") throw new CordisError(1, "HTTP 示例处理错误");
        if (path === "/wait") await new Promise(resolve => setTimeout(resolve, 60_000));
        if (path === "/invalid-status") return {status: 700, headers: [], body: {kind: "empty"}};
        if (path === "/version") return response({label: this.label});
        const chunks: Uint8Array[] = [];
        if (request.body.kind === "inline") chunks.push(new Uint8Array(request.body.value));
        else if (request.body.kind === "stream") await copyStream(request.body.value.source, bytes => chunks.push(bytes));
        const result = response({method: request.method, path: request.path, prefix: request.prefix, query: request.query,
            headers: request.headers, peer: request.peer, body: [...Buffer.concat(chunks)], label: this.label});
        result.headers.push(["set-cookie", "session=one; HttpOnly; Secure"], ["set-cookie", "locale=zh; Secure"], ["connection", "keep-alive"]);
        return result;
    }
}

class Directory {
    constructor(readonly transport: Remote) {}
    async path(): Promise<string> { return await this.transport.invoke("cordis.data-dir@1.0", "path", [], false) as string; }
}

async function apply(context: Context, config: Record<string, any>): Promise<void> {
    const http = await context.get("cordis.http@1.0", api, api.HttpClient);
    const directory = await context.get("cordis.data-dir@1.0", api, Directory);
    let path: string;
    try { path = await directory.path(); } finally { close(directory); }
    const spec: api.RouteSpec = {prefix: config.prefix ?? "/api", methods: config.methods ?? ["GET", "POST", "PUT", "DELETE"],
        maxBodyBytes: config.max_body_bytes === undefined ? null : BigInt(config.max_body_bytes),
        timeoutMs: config.timeout_ms === undefined ? null : BigInt(config.timeout_ms)};
    await registerRoute(context, http, spec, new Handler(path, config.label ?? "typescript"));
}

await run("http-echo-ts", [{name: "http-echo", apply, inject: {"cordis.http@1.0": true, "cordis.data-dir@1.0": true}}]);
