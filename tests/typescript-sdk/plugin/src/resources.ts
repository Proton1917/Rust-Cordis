/** DOC-15 AU-18、DOC-16 §6/§10：资源转交、调用取消和父子效应生命周期。 */
import assert from "node:assert/strict";
import {writeFile} from "node:fs/promises";
import {setTimeout} from "node:timers/promises";
import {BindingTransport, Context, CordisError, close, currentSignal, withSignal, run} from "cordis-sdk";
import * as binding from "./resources-binding.js";

class Item {
    constructor(readonly value: string, readonly evidence: string) {}
    read(): string { return this.value; }
    echo(value: binding.ItemClient): binding.ItemClient { return value; }
    async wait(): Promise<string> {
        try { await setTimeout(20000, undefined, {signal: currentSignal()}); return this.value; }
        finally { await writeFile(`${this.evidence}/cancelled.txt`, "cancelled\n"); }
    }
}

class Provider {
    constructor(readonly evidence: string) {}
    make(value: string): Item { return new Item(value, this.evidence); }
    async borrowValue(value: binding.ItemClient): Promise<string> {
        try { return await value.read(); } finally { close(value); }
    }
    mirror(value: binding.ItemClient): binding.ItemClient { return value; }
}

async function provide(context: Context, config: {evidence: string}): Promise<void> {
    await context.provide("resources@1.0", new Provider(config.evidence), binding);
    const effect = await context.effect(async scope => {
        await scope.defer(async () => { await writeFile(`${config.evidence}/child-cleanup.txt`, "cleaned\n"); });
    });
    effect.close();
}

async function consume(context: Context, config: {evidence: string}): Promise<void> {
    try {
        const service = await context.get("resources@1.0", binding, binding.ResourcesClient);
        const item = await service.make("owned");
        assert.equal(await item.read(), "owned");
        assert.equal(await service.borrowValue(item), "owned");
        const mirrored = await service.mirror(item);
        assert.equal(await mirrored.read(), "owned");
        const echoed = await mirrored.echo(item);
        assert.equal(await echoed.read(), "owned");
        const event = Object.values(binding.SCHEMA.types).find(spec => spec.class === "Offered")!;
        const transport = new BindingTransport(context.runtime, binding);
        let delivered!: () => void;
        const received = new Promise<void>(resolve => { delivered = resolve; });
        await context.on("resources.offered@1.0", async (_context, wire) => {
            const value = binding.decodeValue(event, wire, transport) as binding.Offered;
            assert.equal(await value.item.read(), "owned");
            close(value.item);
            delivered();
        }, false, binding);
        await context.emit("resources.offered@1.0", binding.encodeValue(event, {item: echoed}, transport));
        await received;
        const abort = new AbortController();
        const waiting = assert.rejects(withSignal(abort.signal, () => item.wait()),
            error => error instanceof CordisError && error.code === 7);
        await setTimeout(100);
        abort.abort();
        await waiting;
        assert.equal(await item.read(), "owned");
        await assert.rejects(service.make("x".repeat(context.runtime.connection.limits.maxFrame)),
            error => error instanceof CordisError && error.code === 8);
        close(item);
        assert.equal(await mirrored.read(), "owned");
        close(mirrored);
        close(echoed);
        close(service);
        await writeFile(`${config.evidence}/observed.json`, JSON.stringify({resource: true, borrow: true,
            mirror: true, nestedForward: true, eventResource: true, cancel: true, oversize: true, child: true}));
    } catch (error) {
        await writeFile(`${config.evidence}/failure.json`, JSON.stringify({error: String(error)}));
        throw error;
    }
}

async function parent(context: Context, config: {evidence: string}): Promise<void> {
    const isolated = await context.isolate("resources@1.0");
    await isolated.plugin("child", config);
    await isolated.plugin("consumer", config);
    isolated.close();
}

await run("typescript-resources", [
    {name: "child", apply: provide, provides: ["resources@1.0"]},
    {name: "consumer", apply: consume, inject: {"resources@1.0": true}, listens: ["resources.offered@1.0"]},
    {name: "parent", apply: parent},
]);
