import "cbor-x";

// cbor-x 1.6.0 的运行时导出名称见 index.js 与 decode.js。
declare module "cbor-x" {
    export function setSizeLimits(options: SizeLimitOptions): void;
}
