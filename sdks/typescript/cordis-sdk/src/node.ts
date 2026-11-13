/** DOC-16 §2、§10：Node 标准流与异步调用上下文接入可移植 Runtime。 */
import { AsyncLocalStorage } from "node:async_hooks";
import { Runtime, configureInvocation } from "./runtime.js";
import type { Component } from "./context.js";
import type { Limits } from "./wire.js";

const invocation = new AsyncLocalStorage<AbortSignal>();
configureInvocation({
    current: () => invocation.getStore(),
    run: (signal, operation) => invocation.run(signal, operation),
});

export async function run(
    name: string,
    components: Component[],
    version = "0.1.0",
    limits: Partial<Limits> = {},
): Promise<void> {
    const runtime = new Runtime(name, version, components, process.stdin, process.stdout, limits);
    try {
        await runtime.connection.serve();
    } catch (error) {
        console.error(error);
        process.exitCode = 1;
    } finally {
        process.exit(process.exitCode ?? 0);
    }
}
