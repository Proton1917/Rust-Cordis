/** ADR-0046 §3.8：Node 文件句柄由流对象持有，读到结束或出口归零时关闭。 */
import {closeSync, openSync, readSync, writeSync} from "node:fs";
import {OwnedResource} from "./resources.js";
import {readSize, copyStream, type ByteStream} from "./streams.js";

class FileStream extends OwnedResource implements ByteStream {
    private file: number | undefined;
    constructor(path: string) { super(); this.file = openSync(path, "r"); }
    async read(maxBytes: number): Promise<Uint8Array | null> {
        readSize(maxBytes);
        if (this.file === undefined) return null;
        const bytes = new Uint8Array(maxBytes);
        const count = readSync(this.file, bytes);
        if (count === 0) { this.close(); return null; }
        return bytes.subarray(0, count);
    }
    close(): void {
        if (this.file !== undefined) { closeSync(this.file); this.file = undefined; }
    }
}

export function fromFile(path: string): ByteStream & OwnedResource { return new FileStream(path); }

export async function streamToFile(source: ByteStream, path: string, chunkBytes = 1024 * 1024): Promise<number> {
    const file = openSync(path, "w");
    try {
        return await copyStream(source, bytes => {
            let offset = 0;
            while (offset < bytes.byteLength) offset += writeSync(file, bytes, offset);
        }, chunkBytes);
    } finally { closeSync(file); }
}
