import assert from "node:assert/strict";
import {PassThrough} from "node:stream";
import {test} from "node:test";
import {setTimeout as delay} from "node:timers/promises";
import "../../sdks/typescript/cordis-sdk/dist/node.js";
import {Runtime, withSignal} from "../../sdks/typescript/cordis-sdk/dist/runtime.js";

function fixture(limits = {}) {
    const input = new PassThrough();
    const output = {write(_bytes, done) { queueMicrotask(() => done()); }};
    const runtime = new Runtime("terminal-test", "1", [], input, output, limits);
    const serving = runtime.connection.serve().then(() => undefined, error => error);
    const frame = message => {
        const bytes = runtime.connection.encoder.encode(message);
        const header = Buffer.alloc(4);
        header.writeUInt32BE(bytes.length);
        return Buffer.concat([header, bytes]);
    };
    const reply = (id, result) => frame({response: {id, result}});
    return {runtime, input, reply, async close() {
        input.end();
        const error = await serving;
        if (error) assert.equal(error.code, 6);
    }};
}

test("same-chunk duplicate responses release the unclaimed payload", async () => {
    const f = fixture();
    try {
        const result = f.runtime.connection.request("probe", {});
        f.input.write(Buffer.concat([
            f.reply(1n, {ok: {value: 1, refs: [11n]}}),
            f.reply(1n, {ok: {value: 2, refs: [12n]}}),
        ]));
        const payload = await result;
        assert.deepEqual([...f.runtime.imports.keys()], [11n]);
        payload.close();
        assert.equal(f.runtime.imports.size, 0);
        assert.equal(f.runtime.connection.pending.size, 0);
        assert.equal(f.runtime.connection.closed, false);
    } finally { await f.close(); }
});

test("cancellation claims pending before a same-turn late response", async () => {
    const f = fixture();
    const abort = new AbortController();
    try {
        const result = withSignal(abort.signal, () => f.runtime.connection.request("probe", {}));
        const rejected = assert.rejects(result, {code: 7});
        abort.abort();
        f.input.write(f.reply(1n, {ok: {value: 1, refs: [11n]}}));
        await rejected;
        await delay(0);
        assert.equal(f.runtime.connection.pending.size, 0);
        assert.equal(f.runtime.imports.size, 0);
    } finally { await f.close(); }
});

test("an accepted response survives its former timeout", async () => {
    const f = fixture({timeoutMs: 10, consecutiveTimeouts: 1});
    try {
        const result = f.runtime.connection.request("probe", {});
        f.input.write(f.reply(1n, {ok: {value: 1, refs: [11n]}}));
        const payload = await result;
        await delay(20);
        assert.equal(f.runtime.connection.closed, false);
        assert.deepEqual([...f.runtime.imports.keys()], [11n]);
        payload.close();
    } finally { await f.close(); }
});

test("three remote deadline outcomes retire the connection", async () => {
    const f = fixture({consecutiveTimeouts: 3});
    try {
        for (let id = 1n; id <= 3n; id++) {
            const rejected = assert.rejects(f.runtime.connection.request("probe", {}), {code: 6});
            f.input.write(f.reply(id, {error: {code: 6, message: "remote deadline", detail: null}}));
            await rejected;
            assert.equal(f.runtime.connection.closed, id === 3n);
        }
    } finally { await f.close(); }
});

test("a timely business error resets the timeout streak", async () => {
    const f = fixture({consecutiveTimeouts: 3});
    try {
        let id = 1n;
        for (const code of [6, 1001, 6, 6]) {
            const rejected = assert.rejects(f.runtime.connection.request("probe", {}), {code});
            f.input.write(f.reply(id++, {error: {code, message: "outcome", detail: null}}));
            await rejected;
            assert.equal(f.runtime.connection.closed, false);
        }
    } finally { await f.close(); }
});
