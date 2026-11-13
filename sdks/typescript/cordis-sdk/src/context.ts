/** DOC-15 §6.4、DOC-16 §7–8：组件加载视图与核心拥有的效应。 */
import { BindingTransport, Remote, Service, outgoing, type Reference } from "./bindings.js";
import {
    CordisError,
    configuration,
    fields,
    identifier,
    type Binding,
    type Identifier,
    type MaybePromise,
} from "./types.js";
import type { Runtime } from "./runtime.js";

export interface Component {
    name: string;
    apply(context: Context, config: any): MaybePromise<void>;
    inject?: Record<string, boolean>;
    provides?: string[];
    listens?: string[];
    stateSchema?: string;
    stateVersionMin?: number;
    stateVersionMax?: number;
    resolve?(config: any): MaybePromise<void>;
}

export function defineComponent(component: Component): Component {
    return component;
}

export class Frozen {
    frozen = false;
    check(): void {
        if (this.frozen) throw new CordisError(4, "状态导出期间写入冻结");
    }
}

export class Callback {
    constructor(
        readonly function_: (context: Context, payload?: unknown) => MaybePromise<void>,
        readonly context: Context,
        readonly event = false,
        readonly binding?: Binding,
    ) {}
}
export class Exporter {
    constructor(
        readonly function_: () => MaybePromise<unknown>,
        readonly frozen: Frozen,
        readonly thaw?: () => MaybePromise<void>,
    ) {}
}

const effects = new FinalizationRegistry<{ runtime: Runtime; number: Identifier }>(
    ({ runtime, number }) => {
        runtime.connection.notifySoon("effect_release", { effect: number });
    },
);

/** DOC-16 §8：close 归还外部句柄，dispose 等待核心完成真实效应回退。 */
export class Effect {
    private active = true;
    constructor(
        readonly runtime: Runtime,
        readonly number: Identifier,
    ) {
        effects.register(this, { runtime, number }, this);
    }
    async settled(): Promise<void> {
        const payload = await this.runtime.connection.request("effect_settled", {
            effect: this.number,
        });
        payload.close();
    }
    async dispose(): Promise<void> {
        const payload = await this.runtime.connection.request("effect_dispose", {
            effect: this.number,
        });
        payload.close();
    }
    close(): void {
        if (!this.active) return;
        this.active = false;
        effects.unregister(this);
        this.runtime.connection.notifySoon("effect_release", { effect: this.number });
    }
    [Symbol.dispose](): void {
        this.close();
    }
}

const views = new FinalizationRegistry<Reference>((reference) => reference.release());

export class Context {
    constructor(
        readonly runtime: Runtime,
        readonly number: Identifier,
        readonly config: any,
        readonly inherited: any,
        readonly scope = 0n,
        readonly reference?: Reference,
        readonly signal?: AbortSignal,
    ) {
        if (reference) {
            reference.retain();
            views.register(this, reference, this);
        }
    }

    private async request(method: string, params: object = {}): Promise<any> {
        const payload = await this.runtime.connection.request(method, {
            context: this.number,
            ...params,
        });
        try {
            return payload.value;
        } finally {
            payload.close();
        }
    }

    private async register(
        method: string,
        resource: object,
        key: string,
        params: object,
    ): Promise<any> {
        const number = this.runtime.offer(resource);
        const payload = await this.runtime.connection.request(
            method,
            {
                context: this.number,
                [key]: number,
                refs: [number],
                ...params,
            },
            [number],
        );
        try {
            return payload.value;
        } finally {
            payload.close();
        }
    }

    async provide(
        interfaceName: string,
        provider: object,
        binding: Binding,
        scoped = false,
    ): Promise<void> {
        const service = new Service(provider, binding, interfaceName, scoped);
        await this.register("provide", service, "object", {
            interface: interfaceName,
            methods: service.methods,
            scoped,
        });
    }

    async get<T>(
        interfaceName: string,
        binding: Binding,
        client: new (transport: Remote) => T,
    ): Promise<T> {
        const payload = await this.runtime.connection.request("get", {
            context: this.number,
            interface: interfaceName,
        });
        try {
            return new client(
                new Remote(
                    this.runtime,
                    binding,
                    payload.reference(fields(payload.value).object),
                    interfaceName,
                ),
            );
        } finally {
            payload.close();
        }
    }

    service(
        interfaceName: string,
        provider: object,
        binding: Binding,
    ): { tag: number; value: unknown } {
        return new BindingTransport(this.runtime, binding).exportResource(
            provider,
            interfaceName,
            false,
        );
    }

    async effect(
        body: (context: Context) => MaybePromise<void>,
        label = "typescript-effect",
        asynchronous = false,
    ): Promise<Effect> {
        const result = fields(
            await this.register("effect", new Callback(body, this), "body", {
                label,
                asynchronous,
            }),
        );
        return new Effect(this.runtime, identifier(result.effect));
    }

    async defer(
        cleanup: (context: Context) => MaybePromise<void>,
        asynchronous = true,
    ): Promise<void> {
        if (!this.scope) throw new CordisError(2, "defer 需要效应回调作用域");
        await this.register("defer", new Callback(cleanup, this), "callback", {
            scope: this.scope,
            asynchronous,
        });
    }

    async on(
        event: string,
        callback: (context: Context, payload: unknown) => MaybePromise<void>,
        asynchronous = false,
        binding?: Binding,
    ): Promise<void> {
        await this.register("on", new Callback(callback, this, true, binding), "callback", {
            event,
            asynchronous,
        });
    }

    async emit(event: string, value: unknown = null, asynchronous = false): Promise<void> {
        const offers: Identifier[] = [];
        const payload = await this.runtime.connection.request(
            "emit",
            {
                context: this.number,
                event,
                payload: outgoing(this.runtime, value, offers),
                asynchronous,
                refs: offers,
            },
            offers,
        );
        payload.close();
    }

    async exportState(
        schema: string,
        version: number,
        exporter: () => MaybePromise<unknown>,
        frozen = new Frozen(),
        thaw?: () => MaybePromise<void>,
    ): Promise<Frozen> {
        await this.register("export_state", new Exporter(exporter, frozen, thaw), "exporter", {
            schema,
            version,
        });
        return frozen;
    }

    async isCurrent(): Promise<boolean> {
        return this.request("is_current");
    }
    async inheritedState(): Promise<any> {
        return this.request("inherited_state");
    }
    async configuration(): Promise<any> {
        return configuration(fields(await this.request("config")).value);
    }
    async log(message: string, level = 2): Promise<void> {
        await this.request("log", { message, level });
    }

    private async derive(isolate: object[], intercept: object[]): Promise<Context> {
        const payload = await this.runtime.connection.request("context_derive", {
            context: this.number,
            isolate,
            intercept,
        });
        try {
            const value = fields(payload.value);
            return new Context(
                this.runtime,
                identifier(value.context),
                this.config,
                this.inherited,
                0n,
                payload.reference(value.object),
            );
        } finally {
            payload.close();
        }
    }
    isolate(interfaceName: string, realm: string | null = null): Promise<Context> {
        return this.derive([{ interface: interfaceName, realm }], []);
    }
    intercept(interfaceName: string, meta: unknown): Promise<Context> {
        return this.derive([], [{ interface: interfaceName, meta }]);
    }
    async plugin(component: string, config: unknown = null): Promise<Identifier> {
        return identifier(fields(await this.request("plugin", { component, config })).instance);
    }

    /** DOC-16 §8：任务接受取消信号，效应清理等待后台执行结束。 */
    async task(
        function_: (signal: AbortSignal) => Promise<void>,
        label = "typescript-task",
    ): Promise<Effect> {
        return this.effect(async (scope) => {
            const abort = new AbortController();
            const pending = function_(abort.signal);
            await scope.defer(async () => {
                abort.abort();
                try {
                    await pending;
                } catch (error) {
                    if (!(error instanceof Error) || error.name !== "AbortError") throw error;
                }
            });
        }, label);
    }

    close(): void {
        if (this.reference && views.unregister(this)) this.reference.release();
    }
    [Symbol.dispose](): void {
        this.close();
    }
}
