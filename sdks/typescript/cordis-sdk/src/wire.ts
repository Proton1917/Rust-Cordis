/** DOC-16 §2–6、§10：有界标准流帧、双向请求和连接级所有权。 */
import { Decoder, Encoder, setSizeLimits } from "cbor-x";
import { CordisError, fields, identifier, type Identifier, type Fields } from "./types.js";
import type { Payload } from "./bindings.js";
import { currentSignal, supportsInvocationBudget, type Runtime } from "./runtime.js";
import { checkValue, type ValueLimits } from "./budget.js";

interface Pending {
    ownsDeadline: boolean;
    resolve(value: Payload): void;
    reject(error: unknown): void;
}
export interface Input extends AsyncIterable<Uint8Array> {
    destroy(error?: Error): void;
}
export interface Output {
    write(value: Uint8Array, callback: (error?: Error | null) => void): unknown;
}
export interface Limits extends ValueLimits {
    maxFrame: number;
    maxInflight: number;
    queueCapacity: number;
    timeoutMs: number;
    consecutiveTimeouts: number;
}
export const DEFAULT_LIMITS: Limits = {
    maxFrame: 16777216,
    maxInflight: 64,
    queueCapacity: 64,
    timeoutMs: 30000,
    consecutiveTimeouts: 3,
    maxDepth: 64,
    maxCollection: 65536,
    maxValues: 262144,
};

export class Connection {
    readonly limits: Limits;
    readonly pending = new Map<Identifier, Pending>();
    readonly running = new Map<Identifier, { method: string; abort: AbortController }>();
    readonly decoder = new Decoder({ mapsAsObjects: false });
    readonly encoder = new Encoder({
        useRecords: false,
        mapsAsObjects: false,
        tagUint8Array: false,
    });
    closed = false;
    private next = 1n;
    private timeouts = 0;
    private writes = 0;
    private tail: Promise<void> = Promise.resolve();
    private peerBudget = false;
    private readonly deadlines = new WeakMap<AbortSignal, number>();

    constructor(
        readonly runtime: Runtime,
        readonly input: Input,
        readonly output: Output,
        limits: Partial<Limits>,
    ) {
        this.limits = { ...DEFAULT_LIMITS, ...limits };
        setSizeLimits({
            maxArraySize: this.limits.maxCollection + 1,
            maxMapSize: this.limits.maxCollection + 1,
            maxObjectSize: this.limits.maxCollection + 1,
        });
    }

    /** DOC-16 §6：队列容量和编码失败归还本轮出口，写入失败终止连接。 */
    async send(message: object, offers: Identifier[] = [], beginWrite?: () => boolean): Promise<void> {
        let committed = false;
        try {
            if (this.closed) throw new CordisError(4, "核心连接已关闭");
            if (this.writes >= this.limits.queueCapacity)
                throw new CordisError(8, "发送队列额度耗尽");
            checkValue(message, this.limits);
            const data = this.encoder.encode(message);
            if (data.length > this.limits.maxFrame) throw new CordisError(8, "消息超过帧长度上限");
            const framed = new Uint8Array(data.length + 4);
            new DataView(framed.buffer).setUint32(0, data.length);
            framed.set(data, 4);
            this.writes++;
            const write = this.tail
                .then(async () => {
                    if (this.closed) throw new CordisError(4, "核心连接已关闭");
                    if (beginWrite && !beginWrite()) return;
                    await new Promise<void>((resolve, reject) => {
                        this.output.write(framed, (error) => (error ? reject(error) : resolve()));
                    });
                    committed = true;
                })
                .finally(() => {
                    this.writes--;
                });
            this.tail = write;
            await write;
        } finally {
            if (!committed) this.runtime.releaseExports(offers);
        }
    }

    /** DOC-16 §10：超时释放等待表并交回 cancel；迟到载荷进入释放路径。 */
    async request(method: string, params: object, offers: Identifier[] = []): Promise<Payload> {
        const signal = currentSignal();
        if (signal?.aborted) {
            this.runtime.releaseExports(offers);
            throw signal.reason instanceof CordisError ? signal.reason : new CordisError(7, "调用已取消");
        }
        if (this.pending.size >= this.limits.maxInflight) {
            this.runtime.releaseExports(offers);
            throw new CordisError(8, "在途请求额度耗尽");
        }
        const id = this.next++;
        const started = performance.now();
        const local = started + this.limits.timeoutMs;
        const deadline = Math.min(local, (signal && this.deadlines.get(signal)) ?? local);
        let writing = false;
        let cancelled = false;
        let received: Payload | undefined;
        let timer: ReturnType<typeof setTimeout>;
        let cancel: () => void;
        let expire: () => void;
        const result = new Promise<Payload>((resolve, reject) => {
            this.pending.set(id, {
                ownsDeadline: deadline === local,
                resolve(payload) { received = payload; resolve(payload); },
                reject,
            });
            cancel = () => {
                const pending = this.claimPending(id);
                if (!pending) return;
                cancelled = true;
                pending.reject(signal?.reason instanceof CordisError ? signal.reason : new CordisError(7, "调用已取消"));
                if (writing) this.notifySoon("cancel", { id });
            };
            signal?.addEventListener("abort", cancel, { once: true });
            expire = () => {
                const pending = this.claimPending(id, true);
                if (!pending) return;
                cancelled = true;
                pending.reject(new CordisError(6, "请求超过截止时间"));
                if (writing) this.notifySoon("cancel", { id });
            };
            timer = setTimeout(expire, Math.max(0, deadline - started));
        });
        // 截止时间同时覆盖写入等待，结果 Promise 从创建起即具有失败接收方。
        const completed = result.finally(() => {
            clearTimeout(timer);
            signal?.removeEventListener("abort", cancel);
        });
        try {
            if (method === "invoke" && this.peerBudget) {
                params = { ...params, budget_ms: Math.max(1, Math.floor(deadline - performance.now())) };
            }
            const sent = this.send({ request: { id, method, params } }, offers, () => {
                if (performance.now() >= deadline) expire();
                writing = !cancelled && this.pending.has(id);
                return writing;
            });
            const [, payload] = await Promise.all([sent, completed]);
            return payload;
        } catch (error) {
            this.claimPending(id)?.reject(error);
            received?.close();
            throw error;
        }
    }

    notify(method: string, params: object): Promise<void> {
        return this.send({ notification: { method, params } });
    }

    /** DOC-16 §6：析构控制消息发送失败使连接退役并释放整表。 */
    notifySoon(method: string, params: object): void {
        if (!this.closed) void this.notify(method, params).catch((error) => this.fail(error));
    }

    fail(error: unknown, report = true): void {
        if (this.closed) return;
        this.closed = true;
        for (const pending of this.pending.values()) pending.reject(error);
        this.pending.clear();
        for (const running of this.running.values()) running.abort.abort(error);
        this.runtime.close();
        this.input.destroy(
            report ? (error instanceof Error ? error : new Error(String(error))) : undefined,
        );
    }

    /** CP-10：流结束释放资源；帧长度检查发生在等待完整消息之前。 */
    async serve(): Promise<void> {
        const header = new Uint8Array(4);
        const view = new DataView(header.buffer);
        let headerBytes = 0;
        let length = 0;
        let partial: Uint8Array | undefined;
        let received = 0;
        try {
            for await (const chunk of this.input) {
                let offset = 0;
                while (offset < chunk.length) {
                    if (headerBytes < 4) {
                        const count = Math.min(4 - headerBytes, chunk.length - offset);
                        header.set(chunk.subarray(offset, offset + count), headerBytes);
                        offset += count;
                        headerBytes += count;
                        if (headerBytes < 4) continue;
                        length = view.getUint32(0);
                        if (length > this.limits.maxFrame)
                            throw new CordisError(8, "输入帧超过长度上限");
                    }
                    const count = Math.min(length - received, chunk.length - offset);
                    let bytes = chunk.subarray(offset, offset + count);
                    offset += count;
                    if (received || count < length) {
                        if (!count) continue;
                        const needed = received + count;
                        if (!partial || partial.length < needed) {
                            const grown = new Uint8Array(Math.min(length, Math.max(needed, received * 2)));
                            if (partial) grown.set(partial.subarray(0, received));
                            partial = grown;
                        }
                        partial.set(bytes, received);
                        received = needed;
                        if (received < length) continue;
                        bytes = partial;
                    }
                    const message = this.decoder.decode(bytes);
                    checkValue(message, this.limits);
                    headerBytes = received = 0;
                    partial = undefined;
                    this.receive(message);
                }
            }
            if (headerBytes) throw new CordisError(2, "输入帧被截断");
        } finally {
            this.fail(new CordisError(4, "核心输入已结束"), false);
        }
    }

    private receive(message: unknown): void {
        if (!(message instanceof Map) || message.size !== 1)
            throw new CordisError(2, "消息包络需要单键映射");
        const [kind, entry] = [...message][0];
        const body = fields(entry);
        if (kind === "response") this.response(body);
        else if (kind === "request") this.start(body);
        else if (kind === "notification") {
            void this.runtime
                .notification(body.method, body.params)
                .catch((error) => this.fail(error));
        } else throw new CordisError(3, "消息类别无效");
    }

    /** DOC-16 §10：终态先消费等待项，再更新由本调用拥有的连接健康。 */
    private claimPending(id: Identifier, timedOut?: boolean): Pending | undefined {
        const pending = this.pending.get(id);
        if (!pending) return undefined;
        this.pending.delete(id);
        if (timedOut === false) this.timeouts = 0;
        else if (timedOut && pending.ownsDeadline && ++this.timeouts >= this.limits.consecutiveTimeouts)
            this.fail(new CordisError(6, "连续请求超时"));
        return pending;
    }

    private response(body: Fields): void {
        const id = identifier(body.id);
        const result = fields(body.result);
        if ("error" in result) {
            const error = fields(result.error);
            this.claimPending(id, error.code === 6)?.reject(new CordisError(error.code, error.message, error.detail));
        } else if ("ok" in result) {
            const payload = this.runtime.payload(result.ok);
            const pending = this.claimPending(id, false);
            if (pending) pending.resolve(payload);
            else payload.close();
        } else throw new CordisError(2, "响应需要 ok 或 error");
    }

    private start(body: Fields): void {
        const id = identifier(body.id);
        if (this.running.has(id)) throw new CordisError(2, "请求编号重复");
        if (this.running.size >= this.limits.maxInflight)
            throw new CordisError(8, "入站请求额度耗尽");
        const abort = new AbortController();
        let timer: ReturnType<typeof setTimeout> | undefined;
        if (body.method === "invoke" && supportsInvocationBudget()) {
            const budget = fields(body.params).budget_ms;
            if (budget !== undefined) {
                const integer = typeof budget === "bigint" || Number.isSafeInteger(budget);
                if (!integer || budget < 0 || budget > 0xFFFF_FFFF_FFFF_FFFFn)
                    throw new CordisError(2, "调用预算需要 u64 毫秒值");
                this.peerBudget = true;
                const duration = Math.min(Number(budget), this.limits.timeoutMs);
                this.deadlines.set(abort.signal, performance.now() + duration);
                const expired = () => abort.abort(new CordisError(6, "服务调用超过截止时间"));
                if (duration === 0) expired();
                else timer = setTimeout(expired, duration);
            }
        }
        this.running.set(id, { method: body.method, abort });
        void this.execute(id, body, abort.signal)
            .finally(() => clearTimeout(timer))
            .catch((error) => this.fail(error));
    }

    private async execute(id: Identifier, body: Fields, signal: AbortSignal): Promise<void> {
        const payload = this.runtime.payload(body.params);
        let offers: Identifier[] = [];
        try {
            let result: object;
            try {
                if (signal.aborted) throw signal.reason;
                const response = await this.runtime.request(body.method, payload, signal);
                offers = response.offers;
                const value = body.method === "hello" && supportsInvocationBudget()
                    ? { ...(response.value as object), invoke_budget_v1: true }
                    : response.value;
                result = { ok: value };
            } catch (problem) {
                const error =
                    problem instanceof CordisError
                        ? problem
                        : new CordisError(problem instanceof TypeError ? 2 : 1, String(problem));
                result = { error: error.wire() };
            }
            try {
                await this.send({ response: { id, result } }, offers);
            } catch (problem) {
                if (!(problem instanceof CordisError) || problem.code !== 8) throw problem;
                await this.send({ response: { id, result: { error: problem.wire() } } });
            }
        } finally {
            payload.close();
            this.running.delete(id);
        }
    }
}
