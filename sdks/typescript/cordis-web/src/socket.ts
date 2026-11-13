/** DOC-16 §11：WebSocket 二进制消息与共享 UP/1 标准帧流互相适配。 */
import {CordisError} from "../../cordis-sdk/src/types.js";
import type {Input, Output} from "../../cordis-sdk/src/wire.js";

export class Socket implements Input, Output {
    private readonly frames: Uint8Array[] = [];
    private waiter?: {resolve(value: IteratorResult<Uint8Array>): void; reject(error: unknown): void};
    private ended = false;
    private failure?: Error;

    constructor(readonly socket: WebSocket, readonly maxFrame: number, readonly capacity: number) {
        socket.binaryType = "arraybuffer";
        socket.addEventListener("message", event => this.receive(event.data));
        socket.addEventListener("close", () => this.destroy());
        socket.addEventListener("error", () => this.destroy(new CordisError(4, "网页连接中断")));
    }

    private receive(data: unknown): void {
        if (!(data instanceof ArrayBuffer)) { this.destroy(new CordisError(2, "WebSocket 需要二进制消息")); return; }
        if (data.byteLength > this.maxFrame || this.frames.length >= this.capacity) {
            this.destroy(new CordisError(8, "网页输入超过消息或队列预算")); return;
        }
        const frame = new Uint8Array(data.byteLength + 4);
        new DataView(frame.buffer).setUint32(0, data.byteLength);
        frame.set(new Uint8Array(data), 4);
        if (this.waiter) { const waiter = this.waiter; this.waiter = undefined; waiter.resolve({value: frame, done: false}); }
        else this.frames.push(frame);
    }

    write(frame: Uint8Array, callback: (error?: Error | null) => void): void {
        if (this.ended || this.socket.readyState !== WebSocket.OPEN) { callback(new CordisError(4, "网页连接已关闭")); return; }
        if (this.socket.bufferedAmount + frame.length > this.maxFrame * this.capacity) {
            callback(new CordisError(8, "网页发送队列额度耗尽")); return;
        }
        try { this.socket.send(frame.subarray(4)); callback(); }
        catch (error) { callback(error instanceof Error ? error : new Error(String(error))); }
    }

    [Symbol.asyncIterator](): AsyncIterator<Uint8Array> {
        return {next: () => {
            const frame = this.frames.shift();
            if (frame) return Promise.resolve({value: frame, done: false});
            if (this.failure) return Promise.reject(this.failure);
            if (this.ended) return Promise.resolve({value: undefined, done: true});
            return new Promise((resolve, reject) => { this.waiter = {resolve, reject}; });
        }};
    }

    destroy(error?: Error): void {
        if (this.ended) return;
        this.ended = true; this.failure = error; this.frames.length = 0;
        if (this.waiter) {
            if (error) this.waiter.reject(error); else this.waiter.resolve({value: undefined, done: true});
            this.waiter = undefined;
        }
        if (this.socket.readyState < WebSocket.CLOSING) this.socket.close(error ? 1002 : 1000);
    }
}
