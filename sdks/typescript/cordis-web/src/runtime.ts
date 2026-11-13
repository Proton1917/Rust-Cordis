/** DOC-15 §11.2/§11.4、ADR-0041：页面宿主按核心指令加载独立版本 ES 模块。 */
import {Runtime} from "../../cordis-sdk/src/runtime.js";
import {Callback, type Component} from "../../cordis-sdk/src/context.js";
import {type Payload} from "../../cordis-sdk/src/bindings.js";
import {CordisError, fields, identifier, type Identifier} from "../../cordis-sdk/src/types.js";
import {DEFAULT_LIMITS, type Limits} from "../../cordis-sdk/src/wire.js";
import {Socket} from "./socket.js";

export interface Entry {name: string; version: string; url: string; component: string; mount: string}
interface Module {id: Identifier; entry: Entry; component: Component}

export class WebRuntime extends Runtime {
    private readonly modules = new Map<Identifier, Module>();
    private readonly active = new Map<string, Identifier>();
    private nextModule = 1n;
    private retiredModules = 0;
    private readonly completedCleanups = new WeakSet<Callback>();
    private readonly pendingCleanups = new Map<Callback, Promise<{value: unknown; offers: Identifier[]}>>();
    cleanupComplete: Promise<void> = Promise.resolve();

    constructor(socket: Socket, limits: Partial<Limits> = {}) { super("cordis-web", "0.1.0", [], socket, socket, limits); }

    async load(entry: Entry): Promise<Module> {
        const url = new URL(entry.url, location.href);
        if (url.origin !== location.origin) throw new CordisError(9, "组件入口需要同源地址");
        const namespace = await import(url.href);
        const component: Component = namespace.default;
        if (!component || typeof component.apply !== "function") throw new CordisError(2, "网页模块需要导出组件定义");
        const module = {id: this.nextModule++, entry, component: {...component, name: entry.component}};
        this.modules.set(module.id, module);
        return module;
    }

    async initialize(entries: Entry[]): Promise<void> {
        for (const entry of entries) { const module = await this.load(entry); this.active.set(entry.name, module.id); }
    }

    private descriptor(module: Module): object {
        const component = module.component;
        return {name: module.entry.name, version: module.entry.version, sdk_version: "0.1.0", build_info: new URL(module.entry.url, location.href).pathname,
            format: 2, library: module.id, components: [{
                name: component.name, inject: Object.entries(component.inject ?? {}).map(([interfaceName, required]) => ({interface: interfaceName, required})),
                provides: component.provides ?? [], listens: component.listens ?? [], state_schema: component.stateSchema ?? "",
                state_version_min: component.stateVersionMin ?? 0, state_version_max: component.stateVersionMax ?? 0,
            }]};
    }

    override description(): object {
        return {protocol: 1, plugins: [...this.active.values()].map(id => this.descriptor(this.modules.get(id)!))};
    }

    protected override resolveComponent(params: Record<string, any>): Component {
        const module = this.modules.get(identifier(params.library));
        if (!module || module.entry.name !== params.plugin || module.component.name !== params.component) throw new CordisError(4, "网页模块版本已退役");
        return module.component;
    }

    override async request(method: string, payload: Payload, signal: AbortSignal): Promise<{value: unknown; offers: Identifier[]}> {
        if (method === "callback") {
            const callback = this.object(fields(payload.value).object);
            if (callback instanceof Callback && callback.cleanup) {
                if (this.completedCleanups.has(callback)) return {value: null, offers: []};
                const previous = this.pendingCleanups.get(callback);
                if (previous) return previous;
                const pending = super.request(method, payload, signal).then(result => { this.completedCleanups.add(callback); return result; });
                this.pendingCleanups.set(callback, pending);
                try { return await pending; } finally { this.pendingCleanups.delete(callback); }
            }
        }
        if (!method.startsWith("library_")) return super.request(method, payload, signal);
        const params = fields(payload.value);
        if (method === "library_prepare") {
            const url = new URL(params.path, location.href);
            const entry: Entry = {name: url.searchParams.get("cordis_plugin") ?? "", version: url.searchParams.get("cordis_version") ?? "",
                component: url.searchParams.get("cordis_component") ?? "", mount: url.searchParams.get("cordis_mount") ?? "/", url: url.href};
            const module = await this.load(entry);
            return {value: {library: module.id, plugin: this.descriptor(module)}, offers: []};
        }
        const id = identifier(params.library);
        const module = this.modules.get(id);
        if (!module) throw new CordisError(4, "网页模块编号已退役");
        if (method === "library_activate") { this.active.set(module.entry.name, id); return {value: null, offers: []}; }
        if (method === "library_retire") {
            this.modules.delete(id);
            if (this.active.get(module.entry.name) === id) this.active.delete(module.entry.name);
            return {value: {leases: 0, closed: false, quiescence_ms: 0, retired_images: ++this.retiredModules}, offers: []};
        }
        throw new CordisError(3, "网页模块操作缺席");
    }

    /** DOC-15 AU-15：连接退役执行本宿主尚持有的本地清理，完成后进入下一条连接。 */
    override close(): void {
        const callbacks = [...this.exports.values()].filter((value): value is Callback => value instanceof Callback && value.cleanup).reverse();
        super.close();
        this.cleanupComplete = (async () => {
            for (const callback of callbacks) {
                try {
                    const pending = this.pendingCleanups.get(callback);
                    if (pending) await pending;
                    if (!this.completedCleanups.has(callback)) {
                        await callback.function_(callback.context);
                        this.completedCleanups.add(callback);
                    }
                } catch (error) { console.error("页面资源清理失败", error); }
            }
        })();
    }
}

export {DEFAULT_LIMITS};
