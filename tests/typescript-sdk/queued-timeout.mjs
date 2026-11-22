import assert from "node:assert/strict";
import {PassThrough} from "node:stream";
import {test} from "node:test";
import {setTimeout} from "node:timers/promises";
import {Service} from "../../sdks/typescript/cordis-sdk/dist/bindings.js";
import "../../sdks/typescript/cordis-sdk/dist/node.js";
import {Runtime} from "../../sdks/typescript/cordis-sdk/dist/runtime.js";
import {fields} from "../../sdks/typescript/cordis-sdk/dist/types.js";

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

test("协商后的嵌套调用沿同一连接转发父调用的剩余预算", async () => {
    const requests = new PassThrough();
    const responses = new PassThrough();
    let leafBudget;
    class ObservedRuntime extends Runtime {
        async request(method, payload, signal) {
            if (method === "invoke" && fields(payload.value).method === "leaf")
                leafBudget = fields(payload.value).budget_ms;
            return super.request(method, payload, signal);
        }
    }
    const caller = new ObservedRuntime("caller", "1", [], responses, requests);
    const provider = new Runtime("relay", "1", [], requests, responses);
    const interfaceName = "test.deadline@1.0";
    const binding = {
        SCHEMA: {services: [{interface: interfaceName, methods: [{name: "relay"}, {name: "leaf"}]}]},
        dispatch: (service, _transport, _interface, method, args) => service[method](...args),
    };
    const leaf = caller.offer(new Service({leaf: () => 91}, binding, interfaceName));
    const relay = provider.offer(new Service({
        async relay() {
            await setTimeout(20);
            const child = await provider.connection.request("invoke", {object: leaf, method: "leaf", args: [], refs: []});
            try { return fields(child.value).value; }
            finally { child.close(); }
        },
    }, binding, interfaceName));
    const listening = Promise.all([caller.connection.serve(), provider.connection.serve()]);
    try {
        const hello = await caller.connection.request("hello", {protocol: 1});
        assert.equal(fields(hello.value).invoke_budget_v1, true);
        hello.close();
        const reply = await caller.connection.request("invoke", {object: relay, method: "relay", args: [], refs: [], budget_ms: 1000});
        assert.equal(fields(reply.value).value, 91);
        reply.close();
        assert.ok(leafBudget >= 1 && leafBudget <= 990, `嵌套调用剩余 ${leafBudget} ms`);
    } finally {
        requests.end();
        responses.end();
        await listening;
    }
});
