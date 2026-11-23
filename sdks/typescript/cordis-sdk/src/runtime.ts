/** DOC-16 §5–10：核心指令驱动组件、对象和回调，插件只执行本地函数。 */
import { Connection, type Limits, type Input, type Output } from "./wire.js";
import { BindingTransport, Payload, Reference, Service, outgoing } from "./bindings.js";
import { Callback, Context, Exporter, type Component } from "./context.js";
import { CordisError, configuration, fields, identifier, type Identifier } from "./types.js";

export interface InvocationScope {
    current(): AbortSignal | undefined;
    run<T>(signal: AbortSignal, operation: () => Promise<T>): Promise<T>;
}
let invocation: InvocationScope = {
    current: () => undefined,
    run: (_signal, operation) => operation(),
};
let invocationScopeConfigured = false;
export function configureInvocation(scope: InvocationScope): void {
    invocation = scope;
    invocationScopeConfigured = true;
}
export function supportsInvocationBudget(): boolean {
    return invocationScopeConfigured;
}
export function currentSignal(): AbortSignal | undefined {
    return invocation.current();
}
/** DOC-16 §10：调用方的 AbortSignal 沿本次异步调用交给 cancel 通知。 */
export function withSignal<T>(signal: AbortSignal, operation: () => Promise<T>): Promise<T> {
    return invocation.run(signal, operation);
}

export class Runtime {
    readonly connection: Connection;
    readonly components: Map<string, Component>;
    readonly exports = new Map<Identifier, object>();
    readonly imports = new Map<Identifier, Reference>();
    readonly contexts = new Map<Identifier, Context>();
    readonly events = new Map<Identifier, Set<AbortController>>();
    private next = 1n;
    private activeCallbacks = 0;
    private callbackBytes = 0;

    constructor(
        readonly name: string,
        readonly version: string,
        components: Component[],
        input: Input,
        output: Output,
        limits: Partial<Limits> = {},
    ) {
        this.components = new Map(components.map((component) => [component.name, component]));
        this.connection = new Connection(this, input, output, limits);
    }

    description(): object {
        const components = [...this.components.values()].map((component) => ({
            name: component.name,
            inject: Object.entries(component.inject ?? {}).map(([interfaceName, required]) => ({
                interface: interfaceName,
                required,
            })),
            provides: component.provides ?? [],
            listens: component.listens ?? [],
            state_schema: component.stateSchema ?? "",
            state_version_min: component.stateVersionMin ?? 0,
            state_version_max: component.stateVersionMax ?? 0,
        }));
        return {
            protocol: 1,
            plugins: [
                {
                    name: this.name,
                    version: this.version,
                    sdk_version: "0.2.3",
                    build_info: "TypeScript UP/1 SDK",
                    format: 2,
                    library: null,
                    components,
                },
            ],
        };
    }

    payload(value: unknown): Payload {
        return new Payload(this, value);
    }
    reference(number: Identifier): Reference {
        let reference = this.imports.get(number);
        if (!reference) {
            reference = new Reference(this, number);
            this.imports.set(number, reference);
        }
        return reference;
    }
    offer(resource: object): Identifier {
        if (resource instanceof Service) resource.retain();
        const id = this.next++;
        this.exports.set(id, resource);
        return id;
    }
    releaseExports(numbers: Identifier[]): void {
        for (const number of numbers) {
            const object = this.exports.get(number);
            this.exports.delete(number);
            if (object instanceof Service) object.close();
        }
    }
    object(number: unknown): object {
        const object = this.exports.get(identifier(number));
        if (!object) throw new CordisError(4, "对象已退役");
        return object;
    }

    async request(
        method: string,
        payload: Payload,
        signal: AbortSignal,
    ): Promise<{ value: unknown; offers: Identifier[] }> {
        if (method === "shutdown") return { value: null, offers: [] };
        const params = fields(payload.value);
        let value: unknown = null;
        if (method === "hello") {
            if (params.protocol !== 1) throw new CordisError(3, "需要 UP/1 协议");
            value = this.description();
        } else if (method === "resolve" || method === "apply") await this.component(method, params);
        else if (method === "invoke") {
            const service = this.object(params.object);
            if (!(service instanceof Service)) throw new CordisError(2, "invoke 需要服务对象");
            const result = await invocation.run(signal, () =>
                service.invoke(this, params.method, params.args, payload),
            );
            if (signal.aborted)
                throw signal.reason instanceof CordisError ? signal.reason : new CordisError(7, "调用已取消");
            const offers: Identifier[] = [];
            return { value: { value: outgoing(this, result, offers), refs: offers }, offers };
        } else if (method === "object_info") {
            const service = this.object(params.object);
            if (!(service instanceof Service)) throw new CordisError(2, "object_info 需要服务对象");
            value = { interface: service.interfaceName, methods: service.methods, local: null };
        } else if (method === "callback") await this.callback(params, payload, signal);
        else if (method === "export" || method === "thaw")
            value = await this.state(method, params.object);
        else throw new CordisError(3, `请求方法缺席：${method}`);
        return { value, offers: [] };
    }

    protected resolveComponent(params: Record<string, any>): Component {
        if (params.plugin !== this.name) throw new CordisError(3, "插件名称与进程描述不符");
        const component = this.components.get(params.component);
        if (!component) throw new CordisError(3, "组件名称缺席");
        return component;
    }

    private async component(method: string, params: Record<string, any>): Promise<void> {
        const component = this.resolveComponent(params);
        const config = configuration(params.config);
        if (method === "resolve") {
            await component.resolve?.(config);
            return;
        }
        const context = new Context(this, identifier(params.context), config, params.inherited);
        this.contexts.set(context.number, context);
        await component.apply(context, config);
    }

    private async state(method: string, object: unknown): Promise<unknown> {
        const exporter = this.object(object);
        if (!(exporter instanceof Exporter)) throw new CordisError(2, "状态操作需要导出器");
        if (method === "thaw") {
            exporter.frozen.frozen = false;
            await exporter.thaw?.();
            return null;
        }
        exporter.frozen.frozen = true;
        try {
            return { value: await exporter.function_() };
        } catch (error) {
            exporter.frozen.frozen = false;
            throw error;
        }
    }

    private async callback(
        params: Record<string, any>,
        payload: Payload,
        signal: AbortSignal,
    ): Promise<void> {
        const callback = this.object(params.object);
        if (!(callback instanceof Callback)) throw new CordisError(2, "callback 需要回调对象");
        const base = callback.context;
        const context = new Context(
            this,
            identifier(params.context),
            base.config,
            base.inherited,
            BigInt(params.scope),
            base.reference,
            signal,
        );
        const value = callback.binding
            ? new BindingTransport(this, callback.binding).incoming(params.payload, payload)
            : params.payload;
        try {
            await callback.function_(context, callback.event ? value : undefined);
        } finally {
            context.close();
        }
    }

    /** Reader-measured CBOR notification bodies retained until actual callback completion. */
    get callbackUsage(): Readonly<{ active: number; bytes: number }> {
        return { active: this.activeCallbacks, bytes: this.callbackBytes };
    }

    async notification(method: string, value: unknown, wireBytes?: number): Promise<void> {
        const params = fields(value);
        if (method === "release") this.releaseExports(params.objects.map(identifier));
        else if (method === "cancel") {
            const running = this.connection.running.get(identifier(params.id));
            if (running?.method === "invoke") running.abort.abort(new CordisError(7, "调用已取消"));
        } else if (method === "release_context") {
            const id = identifier(params.context);
            this.contexts.get(id)?.close();
            this.contexts.delete(id);
            for (const abort of this.events.get(id) ?? []) abort.abort();
            this.events.delete(id);
        } else if (method === "callback") this.event(params, value, wireBytes);
        else throw new CordisError(3, `通知方法缺席：${method}`);
    }

    private event(params: Record<string, any>, value: unknown, wireBytes?: number): void {
        const payload = this.payload(value);
        let id: Identifier;
        const size = wireBytes ?? this.connection.limits.maxFrame;
        try {
            id = identifier(params.context);
            if (!Number.isSafeInteger(size) || size <= 0 || size > this.connection.limits.maxFrame)
                throw new CordisError(2, "回调载荷长度无效");
            if (this.activeCallbacks >= this.connection.limits.maxActiveCallbacks ||
                size > this.connection.limits.maxCallbackBytes - this.callbackBytes)
                throw new CordisError(8, "活跃回调额度耗尽");
        } catch (error) {
            payload.close();
            throw error;
        }
        this.activeCallbacks++;
        this.callbackBytes += size;
        const abort = new AbortController();
        let events = this.events.get(id);
        if (!events) {
            events = new Set();
            this.events.set(id, events);
        }
        events.add(abort);
        void this.callback(params, payload, abort.signal)
            .catch((error) => {
                this.connection.notifySoon("log", {
                    context: id,
                    level: 0,
                    message: String(error),
                });
            })
            .finally(() => {
                events.delete(abort);
                payload.close();
                this.activeCallbacks--;
                this.callbackBytes -= size;
                if (!events.size && this.events.get(id) === events) this.events.delete(id);
            });
    }

    close(): void {
        for (const events of this.events.values()) for (const abort of events) abort.abort();
        for (const context of this.contexts.values()) context.close();
        this.releaseExports([...this.exports.keys()]);
        this.events.clear();
        this.contexts.clear();
        this.imports.clear();
    }
}
