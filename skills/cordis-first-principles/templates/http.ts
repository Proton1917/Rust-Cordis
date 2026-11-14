import {Context, registerRoute, run} from "./cordis_sdk/index.js";
import * as api from "./http-bindings.js";

class Handler implements api.HttpHandler {
    async handle(request: api.HttpRequest): Promise<api.HttpResponse> {
        return {status: 200, headers: [["content-type", "text/plain; charset=utf-8"]],
            body: {kind: "inline", value: new TextEncoder().encode(`${request.method} ${request.path}\n`)}};
    }
}

async function apply(context: Context): Promise<void> {
    const http = await context.get("cordis.http@1.0", api, api.HttpClient);
    await registerRoute(context, http, {prefix: "/{{name}}", methods: [], maxBodyBytes: null, timeoutMs: null}, new Handler());
}

await run("{{name}}", [{name: "{{name}}", apply, inject: {"cordis.http@1.0": true}}]);
