/** ADR-0039、DOC-16 §6：生成绑定和拥有型服务引用的适配。 */
import { Tag } from "cbor-x";
import {
    CordisError,
    fields,
    identifier,
    type Binding,
    type Identifier,
    type Transport,
} from "./types.js";
import type { Runtime } from "./runtime.js";
import { OwnedResource } from "./resources.js";

const services = new WeakMap<object, Service>();
const references = new WeakMap<object, Reference>();
const finalizers = new FinalizationRegistry<Reference>((reference) => reference.release());

/** DOC-16 §6：显式份额计数只控制同一核心出口编号的归还。 */
export class Reference {
    private holders = 0;
    constructor(
        readonly runtime: Runtime,
        readonly number: Identifier,
    ) {}
    retain(): this {
        this.holders++;
        return this;
    }
    release(): void {
        if (--this.holders === 0) {
            this.runtime.imports.delete(this.number);
            this.runtime.connection.notifySoon("release", { objects: [this.number] });
        }
    }
}

/** DOC-16 §6：载荷份额独立于已导入服务，close 在请求处理结束执行。 */
export class Payload {
    readonly references = new Map<Identifier, Reference>();
    constructor(
        readonly runtime: Runtime,
        readonly value: any,
    ) {
        if (!(value instanceof Map)) return;
        for (const entry of value.get("refs") ?? []) {
            const number = identifier(entry);
            if (this.references.has(number)) throw new CordisError(2, "refs 中的编号重复");
            this.references.set(number, runtime.reference(number).retain());
        }
    }
    reference(number: unknown): Reference {
        const result = this.references.get(identifier(number));
        if (!result) throw new CordisError(4, "载荷缺少资源持有权");
        return result;
    }
    close(): void {
        for (const reference of this.references.values()) reference.release();
        this.references.clear();
    }
}

export class Service {
    readonly methods: string[];
    constructor(
        readonly provider: object,
        readonly binding: Binding,
        readonly interfaceName: string,
        readonly scoped = false,
    ) {
        const descriptor = binding.SCHEMA.services.find(
            (service) => service.interface === interfaceName,
        );
        if (!descriptor) throw new CordisError(3, `接口描述缺席：${interfaceName}`);
        this.methods = descriptor.methods.map((method) => method.name);
        if (scoped) this.methods.push("cordis.scope");
    }
    exported(): Service {
        const provider = this.provider instanceof Remote ? this.provider.clone() : this.provider;
        return new Service(provider, this.binding, this.interfaceName, this.scoped);
    }
    close(): void {
        if (this.provider instanceof Remote) this.provider.close();
        if (this.provider instanceof OwnedResource) this.provider.release();
    }
    retain(): void {
        if (this.provider instanceof OwnedResource) this.provider.retain();
    }
    async invoke(
        runtime: Runtime,
        method: string,
        args: unknown[],
        payload: Payload,
    ): Promise<unknown> {
        const transport = new BindingTransport(runtime, this.binding);
        if (method === "cordis.scope" && this.scoped) {
            if (args.length !== 1) throw new CordisError(2, "cordis.scope 需要一项元数据");
            const view = await (this.provider as { scope(meta: unknown): object }).scope(args[0]);
            return transport.exportResource(view, this.interfaceName, false);
        }
        if (!this.methods.includes(method)) throw new CordisError(3, "接口方法缺席");
        const values = transport.incoming(args, payload) as unknown[];
        let provider = this.provider;
        if (provider instanceof Remote) {
            const descriptor = this.binding.SCHEMA.services.find(
                (service) => service.interface === this.interfaceName,
            )!;
            const client = (
                this.binding as unknown as Record<string, new (transport: Remote) => object>
            )[`${descriptor.class}Client`];
            provider = new client(provider);
        }
        return this.binding.dispatch(provider, transport, this.interfaceName, method, values);
    }
}

export class BindingTransport implements Transport {
    constructor(
        readonly runtime: Runtime,
        readonly binding: Binding,
    ) {}

    invoke(
        _service: string,
        _method: string,
        _args: unknown[],
        _asynchronous: boolean,
    ): Promise<unknown> {
        return Promise.reject(new CordisError(4, "调用需要已取得的远程服务"));
    }

    exportResource(
        value: unknown,
        service: string,
        _borrowed = false,
    ): { tag: number; value: unknown } {
        const provider = (value as { transport?: object })?.transport ?? value;
        const tag = new this.binding.Tagged(60000, 0);
        services.set(tag, new Service(provider as object, this.binding, service));
        return tag;
    }

    importResource(
        value: { tag: number; value: unknown },
        service: string,
        _borrowed = false,
    ): Remote {
        const reference = references.get(value);
        if (!reference) throw new CordisError(4, "资源标签缺少拥有关系");
        const remote = new Remote(this.runtime, this.binding, reference, service);
        if (finalizers.unregister(value)) reference.release();
        return remote;
    }

    /** ADR-0039：标签保留自身份额，生成 Client 解码后取得独立持有。 */
    incoming(value: any, payload: Payload): any {
        if (value instanceof Tag) {
            const tag = new this.binding.Tagged(value.tag, this.incoming(value.value, payload));
            if (value.tag === 60000) {
                const reference = payload.reference(value.value).retain();
                references.set(tag, reference);
                finalizers.register(tag, reference, tag);
            }
            return tag;
        }
        if (value instanceof Map)
            return new Map(
                Array.from(value, ([key, item]) => [
                    this.incoming(key, payload),
                    this.incoming(item, payload),
                ]),
            );
        if (Array.isArray(value)) return value.map((item) => this.incoming(item, payload));
        return value;
    }
}

/** DOC-16 §6：代理可显式 close，GC 后的最后份额由注册回调归还。 */
export class Remote extends BindingTransport {
    private active = true;
    constructor(
        runtime: Runtime,
        binding: Binding,
        readonly reference: Reference,
        readonly interfaceName: string,
    ) {
        super(runtime, binding);
        reference.retain();
        finalizers.register(this, reference, this);
    }
    clone(): Remote {
        if (!this.active) throw new CordisError(4, "服务代理已释放");
        return new Remote(this.runtime, this.binding, this.reference, this.interfaceName);
    }
    async invoke(
        _service: string,
        method: string,
        args: unknown[],
        _asynchronous = true,
    ): Promise<unknown> {
        if (!this.active) throw new CordisError(4, "服务代理已释放");
        const offers: Identifier[] = [];
        const values = outgoing(this.runtime, args, offers);
        const payload = await this.runtime.connection.request(
            "invoke",
            {
                object: this.reference.number,
                method,
                args: values,
                refs: offers,
            },
            offers,
        );
        try {
            return this.incoming(fields(payload.value).value, payload);
        } finally {
            payload.close();
        }
    }
    close(): void {
        if (!this.active) return;
        this.active = false;
        finalizers.unregister(this);
        this.reference.release();
    }
    [Symbol.dispose](): void {
        this.close();
    }
}

/** DOC-16 §6：转换生成标签，由本轮 offers 持有创建的出口。 */
export function outgoing(runtime: Runtime, value: any, offers: Identifier[]): any {
    try {
        return transform(runtime, value, offers);
    } catch (error) {
        runtime.releaseExports(offers);
        throw error;
    }
}

function transform(runtime: Runtime, value: any, offers: Identifier[]): any {
    if (value && typeof value === "object" && "tag" in value && "value" in value) {
        const service = services.get(value);
        if (service) {
            const number = runtime.offer(service.exported());
            offers.push(number);
            return new Tag(number, 60000);
        }
        if (value.tag === 60000) throw new CordisError(2, "资源标签需要 exportResource");
        return new Tag(transform(runtime, value.value, offers), value.tag);
    }
    if (value instanceof Map)
        return new Map(
            Array.from(value, ([key, item]) => [
                transform(runtime, key, offers),
                transform(runtime, item, offers),
            ]),
        );
    if (Array.isArray(value)) return value.map((item) => transform(runtime, item, offers));
    if (value && typeof value === "object" && !(value instanceof Uint8Array)) {
        return new Map(
            Object.entries(value).map(([key, item]) => [key, transform(runtime, item, offers)]),
        );
    }
    return value;
}

/** ADR-0039：生成 Client 的显式释放入口保留其公开生成方法。 */
export function close(client: object): void {
    const transport =
        client instanceof Remote ? client : (client as { transport?: unknown }).transport;
    if (!(transport instanceof Remote)) throw new CordisError(2, "close 需要远程 Client");
    transport.close();
}
