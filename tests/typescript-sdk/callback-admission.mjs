import assert from "node:assert/strict";
import {PassThrough} from "node:stream";
import {setImmediate} from "node:timers/promises";
import {test} from "node:test";
import {Callback, Context} from "../../sdks/typescript/cordis-sdk/dist/context.js";
import {Runtime} from "../../sdks/typescript/cordis-sdk/dist/runtime.js";
import {fields} from "../../sdks/typescript/cordis-sdk/dist/types.js";

function deferred() {
    let resolve;
    const promise = new Promise((done) => { resolve = done; });
    return {promise, resolve};
}
async function until(predicate) {
    const deadline = performance.now() + 1000;
    while (!predicate()) {
        assert.ok(performance.now() < deadline, "callback lifecycle made no progress");
        await setImmediate();
    }
}
function setup(callback, limits = {}) {
    const input = new PassThrough();
    const frames = [];
    const output = {write(frame, done) { frames.push(frame.slice()); done(); }};
    const runtime = new Runtime("test", "1", [], input, output, limits);
    const id = runtime.offer(new Callback(callback, new Context(runtime, 1n, {}, null), true));
    return {runtime, input, frames, id};
}
function params(object, payload = null, context = 1n, reference = 101n) {
    return {object, context, scope: 0, payload, refs: [reference]};
}
function frame(runtime, message) {
    const body = runtime.connection.encoder.encode(message);
    const value = Buffer.alloc(body.length + 4);
    value.writeUInt32BE(body.length);
    value.set(body, 4);
    return value;
}
function event(runtime, value) {
    return frame(runtime, {notification: {method: "callback", params: value}});
}
function decoded(runtime, value) {
    return runtime.connection.decoder.decode(runtime.connection.encoder.encode(value));
}
async function accept(runtime, value) {
    const bytes = event(runtime, value).length - 4;
    await runtime.notification("callback", decoded(runtime, value), bytes);
    return bytes;
}

test("Payload reference admission validates every entry before retaining", {timeout: 3000}, async () => {
    for (const refs of [[101n, 101n], [101n, 0n]]) {
        for (const shared of [false, true]) {
            const activated = [];
            const {runtime, frames, id} = setup(() => activated.push(true));
            const owner = shared ? runtime.payload(decoded(runtime, {refs: [101n]})) : undefined;
            try {
                await assert.rejects(accept(runtime, {...params(id), refs}), {code: 2});
                assert.deepEqual(runtime.callbackUsage, {active: 0, bytes: 0});
                assert.equal(runtime.events.size, 0);
                assert.deepEqual([...runtime.imports.keys()], shared ? [101n] : []);
                assert.equal(runtime.connection.closed, false);
                await setImmediate();
                assert.deepEqual(activated, []);
                assert.equal(frames.length, 0);
                if (owner) {
                    owner.close();
                    await until(() => frames.length === 1);
                    assert.equal(runtime.imports.size, 0);
                    const notice = fields(fields(runtime.connection.decoder.decode(frames[0].subarray(4))).notification);
                    assert.equal(notice.method, "release");
                    assert.deepEqual(fields(notice.params).objects.map(BigInt), [101n]);
                }
            } finally {
                owner?.close();
                runtime.close();
            }
        }
    }
});

test("Payload reference shares retain their existing lifecycle", {timeout: 3000}, async () => {
    const {runtime, frames} = setup(() => {});
    const first = runtime.payload(decoded(runtime, {refs: [101n, 102n]}));
    const second = runtime.payload(decoded(runtime, {refs: [101n]}));
    const retained = first.reference(102n).retain();
    first.close();
    assert.deepEqual([...runtime.imports.keys()], [101n, 102n]);
    assert.equal(frames.length, 0);
    second.close();
    assert.deepEqual([...runtime.imports.keys()], [102n]);
    retained.release();
    assert.equal(runtime.imports.size, 0);
    first.close();
    second.close();
    await until(() => frames.length === 2);
    const released = frames.map(frame => fields(fields(runtime.connection.decoder.decode(frame.subarray(4))).notification))
        .flatMap(notice => fields(notice.params).objects.map(BigInt));
    assert.deepEqual(released, [101n, 102n]);
    runtime.close();
});

// Every callback uses the real exported Callback and Context implementation.
test("callback count admission returns rejected refs and reuses completed capacity", {timeout: 3000}, async () => {
    const finish = deferred();
    const activated = [];
    const {runtime, frames, id} = setup(async (_context, value) => {
        activated.push(value);
        await finish.promise;
    }, {maxActiveCallbacks: 2});
    const first = await accept(runtime, params(id, 0));
    const second = await accept(runtime, params(id, 1, 1n, 102n));
    await assert.rejects(accept(runtime, params(id, 2, 1n, 103n)), {code: 8});
    assert.deepEqual(runtime.callbackUsage, {active: 2, bytes: first + second});
    assert.deepEqual([...runtime.imports.keys()], [101n, 102n]);
    assert.deepEqual(activated, [0, 1]);
    finish.resolve();
    await until(() => runtime.callbackUsage.active === 0);
    assert.deepEqual(runtime.callbackUsage, {active: 0, bytes: 0});
    assert.equal(runtime.imports.size, 0);
    await accept(runtime, params(id, 3, 1n, 104n));
    await until(() => runtime.callbackUsage.active === 0 && frames.length === 4);
    const released = frames.map((f) => fields(fields(runtime.connection.decoder.decode(f.subarray(4))).notification))
        .filter((body) => body.method === "release")
        .flatMap((body) => fields(body.params).objects.map(BigInt));
    assert.deepEqual(released.sort(), [101n, 102n, 103n, 104n]);
    runtime.close();
});

test("wire callback overload visibly retires the connection and preserves accepted start order", {timeout: 3000}, async () => {
    const finish = deferred();
    const activated = [];
    const {runtime, input, id} = setup(async (_context, value) => {
        activated.push(value);
        await finish.promise;
    }, {maxActiveCallbacks: 2});
    const serving = runtime.connection.serve().then(() => undefined, (error) => error);
    input.write(Buffer.concat([0, 1, 2].map((value) => event(runtime, params(id, value, 1n, 101n + BigInt(value))))));
    const error = await serving;
    assert.equal(error.code, 8);
    assert.equal(runtime.connection.closed, true);
    assert.deepEqual(activated, [0, 1]);
    assert.equal(runtime.callbackUsage.active, 2);
    finish.resolve();
    await until(() => runtime.callbackUsage.active === 0);
    assert.equal(runtime.callbackUsage.bytes, 0);
});

test("callback byte quota uses the actual reader body length at the exact boundary", {timeout: 3000}, async () => {
    const fixture = setup(() => {});
    const wire = event(fixture.runtime, params(fixture.id, new Uint8Array(512)));
    const bytes = wire.length - 4;
    fixture.runtime.close();
    for (const maximum of [bytes, bytes - 1]) {
        const finish = deferred();
        const {runtime, input} = setup(() => finish.promise, {maxCallbackBytes: maximum});
        const serving = runtime.connection.serve().then(() => undefined, (error) => error);
        input.write(wire);
        if (maximum === bytes) {
            await until(() => runtime.callbackUsage.active === 1);
            assert.deepEqual(runtime.callbackUsage, {active: 1, bytes});
            finish.resolve();
            await until(() => runtime.callbackUsage.active === 0);
            input.end();
            assert.equal(await serving, undefined);
        } else {
            assert.equal((await serving).code, 8);
            assert.deepEqual(runtime.callbackUsage, {active: 0, bytes: 0});
        }
    }
});

test("release_context keeps a cancellation-resistant callback charged until it settles", {timeout: 3000}, async () => {
    const finish = deferred();
    let signal;
    const {runtime, id} = setup(async (context) => {
        signal = context.signal;
        await finish.promise;
    }, {maxActiveCallbacks: 1});
    const bytes = await accept(runtime, params(id));
    await runtime.notification("release_context", decoded(runtime, {context: 1n}));
    assert.equal(signal.aborted, true);
    assert.deepEqual(runtime.callbackUsage, {active: 1, bytes});
    assert.equal(runtime.imports.has(101n), true);
    await assert.rejects(accept(runtime, params(id, null, 1n, 102n)), {code: 8});
    finish.resolve();
    await until(() => runtime.callbackUsage.active === 0);
    assert.deepEqual(runtime.callbackUsage, {active: 0, bytes: 0});
    assert.equal(runtime.imports.size, 0);
    assert.equal(runtime.events.size, 0);
    runtime.close();
});

test("reverse replies and context release progress while all callback slots are occupied", {timeout: 3000}, async () => {
    const received = deferred();
    const second = deferred();
    const activated = [];
    const {runtime, input, frames, id} = setup(async (context, value) => {
        activated.push(value);
        if (value === 0) {
            const reply = await runtime.connection.request("probe", {});
            try { assert.equal(reply.value, 91); received.resolve(); }
            finally { reply.close(); }
        } else {
            second.resolve();
            await new Promise((resolve) => context.signal.addEventListener("abort", resolve, {once: true}));
        }
    }, {maxActiveCallbacks: 2});
    const serving = runtime.connection.serve();
    try {
        input.write(Buffer.concat([event(runtime, params(id, 0)), event(runtime, params(id, 1, 2n, 102n))]));
        await second.promise;
        await until(() => frames.length > 0);
        assert.equal(runtime.callbackUsage.active, 2);
        const request = fields(fields(runtime.connection.decoder.decode(frames[0].subarray(4))).request);
        input.write(frame(runtime, {response: {id: request.id, result: {ok: 91}}}));
        await received.promise;
        input.write(frame(runtime, {notification: {method: "release_context", params: {context: 2n}}}));
        await until(() => runtime.callbackUsage.active === 0);
        assert.deepEqual(activated, [0, 1]);
        assert.deepEqual(runtime.callbackUsage, {active: 0, bytes: 0});
        assert.equal(runtime.imports.size, 0);
        assert.equal(runtime.connection.pending.size, 0);
    } finally {
        input.end();
        await serving;
    }
});

test("callback limits accept only positive safe integers", () => {
    for (const limits of [{maxActiveCallbacks: 0}, {maxCallbackBytes: 0},
        {maxActiveCallbacks: 1.5}, {maxCallbackBytes: Number.MAX_SAFE_INTEGER + 1}])
        assert.throws(() => setup(() => {}, limits), {code: 2});
});
