/** DOC-15 §11：连接 HTTP 转发端点，核心统一调度页面组件及热替换。 */
import {DEFAULT_LIMITS, WebRuntime, type Entry} from "./runtime.js";
import {Socket} from "./socket.js";
import {CordisError} from "../../cordis-sdk/src/types.js";

export {Context, Effect, Frozen, defineComponent, type Component} from "../../cordis-sdk/src/context.js";
export {BindingTransport, Remote, close} from "../../cordis-sdk/src/bindings.js";
export {CordisError};
export {caller, type Caller} from "../../cordis-sdk/src/metadata.js";
export {WebRuntime, type Entry};

export interface Options {
    token: string; endpoint?: string; manifest?: string; reconnectAttempts?: number; reconnectDelayMs?: number;
    status?(state: "connecting" | "active" | "disconnected" | "closed", error?: unknown): void;
}

/** CP-10、DOC-15 AU-15：页面关闭回收本次连接；有界重连重新取得页面实例。 */
export function connect(options: Options): {close(): void; completed: Promise<void>} {
    const stop = new AbortController();
    let current: Socket | undefined;
    let wake: (() => void) | undefined;
    const close = () => { stop.abort(); current?.destroy(); wake?.(); };
    const onPageHide = () => close();
    window.addEventListener("pagehide", onPageHide, {once: true});
    const completed = (async () => {
        const attempts = options.reconnectAttempts ?? 20;
        let token = options.token;
        for (let attempt = 0; !stop.signal.aborted && attempt <= attempts; attempt++) {
            options.status?.("connecting");
            let runtime: WebRuntime | undefined;
            try {
                if (attempt > 0) {
                    const session = await fetch("/_cordis/session.json", {cache: "no-store", signal: stop.signal});
                    if (!session.ok) throw new CordisError(session.status, "读取当前会话失败");
                    token = (await session.json()).token;
                }
                const response = await fetch(options.manifest ?? "/_cordis/manifest.json", {headers: {Authorization: `Bearer ${token}`}, cache: "no-store", signal: stop.signal});
                if (!response.ok) throw new CordisError(response.status, "读取网页插件清单失败");
                const entries = await response.json() as Entry[];
                const url = new URL(options.endpoint ?? "/_cordis/socket", location.href);
                url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
                const websocket = new WebSocket(url, ["cordis-up1", `cordis-token.${token}`]);
                current = new Socket(websocket, DEFAULT_LIMITS.maxFrame, DEFAULT_LIMITS.queueCapacity);
                runtime = new WebRuntime(current);
                await initializeSocket(websocket, runtime, entries, stop.signal);
                if (stop.signal.aborted) break;
                options.status?.("active");
                await runtime.connection.serve();
            } catch (error) { current?.destroy(); options.status?.("disconnected", error); }
            finally { await runtime?.cleanupComplete; }
            if (!stop.signal.aborted && attempt < attempts) await new Promise<void>(resolve => {
                const timer = setTimeout(resolve, options.reconnectDelayMs ?? 500);
                wake = () => { clearTimeout(timer); resolve(); };
            });
        }
        window.removeEventListener("pagehide", onPageHide);
        options.status?.("closed");
    })();
    return {close, completed};
}

async function initializeSocket(socket: WebSocket, runtime: WebRuntime, entries: Entry[], stop: AbortSignal): Promise<void> {
    const listeners = new AbortController();
    const options = {signal: listeners.signal};
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
        const ended = new Promise<never>((_, reject) => {
            const disconnected = () => reject(new CordisError(4, "网页连接在初始化期间关闭"));
            socket.addEventListener("close", disconnected, options);
            socket.addEventListener("error", disconnected, options);
            stop.addEventListener("abort", disconnected, options);
            if (stop.aborted || socket.readyState >= WebSocket.CLOSING) disconnected();
            timer = setTimeout(() => reject(new CordisError(6, "网页连接初始化超时")), DEFAULT_LIMITS.timeoutMs);
        });
        const opened = new Promise<void>(resolve => {
            if (socket.readyState === WebSocket.OPEN) resolve();
            else socket.addEventListener("open", () => resolve(), options);
        });
        await Promise.race([ended, Promise.all([opened, runtime.initialize(entries)])]);
    } finally {
        clearTimeout(timer);
        listeners.abort();
    }
}
