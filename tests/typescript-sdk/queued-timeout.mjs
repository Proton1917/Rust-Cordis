import assert from "node:assert/strict";
import {PassThrough} from "node:stream";
import {test} from "node:test";
import {Runtime} from "../../sdks/typescript/cordis-sdk/dist/runtime.js";

test("本地排队的超时请求在恢复写入后归还资源并跳过激活", async () => {
    const requests = new PassThrough();
    const responses = new PassThrough();
    const caller = new Runtime("caller", "1", [], responses, requests, {timeoutMs: 100});
    let activations = 0;
    const receiver = new Runtime("target", "1", [{
        name: "counter", apply() { activations++; },
    }], requests, responses);
    const listening = Promise.all([caller.connection.serve(), receiver.connection.serve()]);
    const apply = {plugin: "target", component: "counter", context: 1n, config: {}, inherited: null};
    try {
        requests.cork();
        const first = caller.connection.notify("release", {objects: []});
        const offered = caller.offer({});
        await assert.rejects(caller.connection.request("apply", apply, [offered]), {code: 6});
        requests.uncork();
        await first;
        const reply = await caller.connection.request("apply", {...apply, context: 2n});
        reply.close();
        assert.equal(activations, 1);
        assert.equal(caller.exports.has(offered), false);
    } finally {
        requests.uncork();
        requests.end();
        responses.end();
        await listening;
    }
});
