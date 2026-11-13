/** DOC-16 §2–6、§10：有界标准流帧、双向请求和连接级所有权。 */
import { Decoder, Encoder, setSizeLimits } from "cbor-x";
import type { Readable, Writable } from "node:stream";
import { CordisError, fields, identifier, type Identifier, type Fields } from "./types.js";
import type { Payload } from "./bindings.js";
import { currentSignal, type Runtime } from "./runtime.js";
import { checkValue, type ValueLimits } from "./budget.js";

interface Pending {
    resolve(value: Payload): void;
    reject(error: unknown): void;
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

    constructor(
        readonly runtime: Runtime,
        readonly input: Readable,
        readonly output: Writable,
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
    async send(message: object, offers: Identifier[] = []): Promise<void> {
        let committed = false;
        try {
            if (this.closed) throw new CordisError(4, "核心连接已关闭");
            if (this.writes >= this.limits.queueCapacity)
                throw new CordisError(8, "发送队列额度耗尽");
            checkValue(message, this.limits);
            const data = this.encoder.encode(message);
            if (data.length > this.limits.maxFrame) throw new CordisError(8, "消息超过帧长度上限");
            const header = Buffer.allocUnsafe(4);
            header.writeUInt32BE(data.length);
            this.writes++;
            const write = this.tail
                .then(async () => {
                    if (this.closed) throw new CordisError(4, "核心连接已关闭");
                    await new Promise<void>((resolve, reject) => {
                        this.output.write(Buffer.concat([header, data]), (error) =>
                            error ? reject(error) : resolve(),
                        );
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
            throw new CordisError(7, "调用已取消");
        }
        if (this.pending.size >= this.limits.maxInflight) {
            this.runtime.releaseExports(offers);
            throw new CordisError(8, "在途请求额度耗尽");
        }
        const id = this.next++;
        let delivered = false;
        let cancelled = false;
        let rejectResponse: (error: unknown) => void;
        let timer: ReturnType<typeof setTimeout>;
        let cancel: () => void;
        const result = new Promise<Payload>((resolve, reject) => {
            rejectResponse = reject;
            this.pending.set(id, { resolve, reject });
            cancel = () => {
                cancelled = true;
                reject(new CordisError(7, "调用已取消"));
                if (delivered) this.notifySoon("cancel", { id });
            };
            signal?.addEventListener("abort", cancel, { once: true });
            timer = setTimeout(() => {
                cancelled = true;
                reject(new CordisError(6, "请求超过截止时间"));
                if (delivered) this.notifySoon("cancel", { id });
                if (++this.timeouts >= this.limits.consecutiveTimeouts)
                    this.fail(new CordisError(6, "连续请求超时"));
            }, this.limits.timeoutMs);
        });
        // 截止时间同时覆盖写入等待，结果 Promise 从创建起即具有失败接收方。
        const completed = result.finally(() => {
            clearTimeout(timer);
            this.pending.delete(id);
            signal?.removeEventListener("abort", cancel);
        });
        try {
            const sent = this.send({ request: { id, method, params } }, offers).then(() => {
                delivered = true;
                if (cancelled) this.notifySoon("cancel", { id });
            });
            const [, payload] = await Promise.all([sent, completed]);
            this.timeouts = 0;
            return payload;
        } catch (error) {
            rejectResponse!(error);
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
        let buffered = Buffer.alloc(0);
        try {
            for await (const chunk of this.input) {
                buffered = Buffer.concat([buffered, chunk]);
                while (buffered.length >= 4) {
                    const length = buffered.readUInt32BE(0);
                    if (length > this.limits.maxFrame)
                        throw new CordisError(8, "输入帧超过长度上限");
                    if (buffered.length < length + 4) break;
                    const message = this.decoder.decode(buffered.subarray(4, length + 4));
                    checkValue(message, this.limits);
                    buffered = buffered.subarray(length + 4);
                    this.receive(message);
                }
            }
            if (buffered.length) throw new CordisError(2, "输入帧被截断");
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

    private response(body: Fields): void {
        const id = identifier(body.id);
        const result = fields(body.result);
        const pending = this.pending.get(id);
        if ("error" in result) {
            const error = fields(result.error);
            pending?.reject(new CordisError(error.code, error.message, error.detail));
        } else if ("ok" in result) {
            const payload = this.runtime.payload(result.ok);
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
        this.running.set(id, { method: body.method, abort });
        void this.execute(id, body, abort.signal).catch((error) => this.fail(error));
    }

    private async execute(id: Identifier, body: Fields, signal: AbortSignal): Promise<void> {
        const payload = this.runtime.payload(body.params);
        let offers: Identifier[] = [];
        try {
            let result: object;
            try {
                const response = await this.runtime.request(body.method, payload, signal);
                offers = response.offers;
                result = { ok: response.value };
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
