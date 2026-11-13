/** ADR-0046 §3.8：通用流以有界块拉取，调用方控制文件与回调的写入节奏。 */
import {close as closeRemote, Remote} from "./bindings.js";
import {OwnedResource} from "./resources.js";
import {CordisError} from "./types.js";

export const MAX_CHUNK_BYTES = 4 * 1024 * 1024;
export interface ByteStream { read(maxBytes: number): Promise<Uint8Array | null | {value: Uint8Array}>; }

export function readSize(size: number): number {
    if (!Number.isInteger(size) || size < 1 || size > MAX_CHUNK_BYTES) {
        throw new CordisError(2, "字节流读取长度需要为 1 至 4194304");
    }
    return size;
}

class Chunks extends OwnedResource implements ByteStream {
    private pending: Uint8Array = new Uint8Array(0);
    private ended = false;
    constructor(private readonly chunks: Iterator<Uint8Array>) { super(); }
    async read(maxBytes: number): Promise<Uint8Array | null> {
        readSize(maxBytes);
        if (this.ended) return null;
        while (!this.pending.byteLength) {
            const item = this.chunks.next();
            if (item.done) { this.close(); return null; }
            this.pending = item.value;
        }
        const chunk = this.pending.subarray(0, maxBytes);
        this.pending = this.pending.subarray(chunk.byteLength);
        return chunk;
    }
    close(): void {
        if (!this.ended) { this.ended = true; this.chunks.return?.(); }
        this.pending = new Uint8Array(0);
    }
}

export function fromChunks(chunks: Iterable<Uint8Array>): ByteStream & OwnedResource {
    return new Chunks(chunks[Symbol.iterator]());
}

export async function copyStream(source: ByteStream, write: (bytes: Uint8Array) => unknown, chunkBytes = 1024 * 1024): Promise<number> {
    readSize(chunkBytes);
    let total = 0;
    try {
        for (;;) {
            const result = await source.read(chunkBytes);
            if (result === null) return total;
            const bytes = result instanceof Uint8Array ? result : result.value;
            if (!bytes.byteLength || bytes.byteLength > chunkBytes) throw new CordisError(2, "字节流块长度超出读取契约");
            await write(bytes);
            total += bytes.byteLength;
        }
    } finally {
        if ((source as {transport?: unknown}).transport instanceof Remote) closeRemote(source);
    }
}
