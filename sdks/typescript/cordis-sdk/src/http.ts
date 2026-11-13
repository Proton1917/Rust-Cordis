/** ADR-0046 §3.5/3.9：路由效应与页面来源查询复用普通服务代理。 */
import type {Context, Effect} from "./context.js";
import {CordisError} from "./types.js";

export async function registerRoute<S, H>(context: Context, http: {
    route(spec: S, handler: H): Promise<bigint>;
    unroute(id: bigint): Promise<void>;
}, spec: S, handler: H): Promise<Effect> {
    const effect = await context.effect(async scope => {
        const id = await http.route(spec, handler);
        try { await scope.defer(async () => { await http.unroute(id); }); }
        catch (error) { await http.unroute(id); throw error; }
    }, "HTTP route", true);
    await effect.settled();
    return effect;
}

export async function pageAttributes<T>(metadata: Map<string, unknown>, pages: {
    attributes(key: Uint8Array): Promise<T>;
}): Promise<T | null> {
    const source = metadata.get("cordis.page");
    if (source === undefined) return null;
    if (!(source instanceof Map) || !source.has("key")) throw new CordisError(2, "页面来源需要页面键");
    return await pages.attributes(new Uint8Array(source.get("key") as ArrayLike<number>));
}
