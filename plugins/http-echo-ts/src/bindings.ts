// DOC-15 §7、ADR-0039：绑定以显式 Transport 管理调用与资源引用。
import { Tagged, Some, bindRuntime, type Option, type ResultValue, type Transport, type Schema } from "cordis-sdk";
export { Tagged, Some, type Option, type ResultValue, type Transport };

export const SCHEMA: Schema = {"package":"cordis:http@1.0.0","services":[{"class":"Streams","interface":"streams@1.0","methods":[]},{"class":"CordisByteStreamStreamsByteStream","interface":"cordis.byte-stream@1.0","methods":[{"async":true,"error":{"kind":"string"},"name":"read","params":[{"name":"max-bytes","type":{"kind":"u32"}}],"python":"read","result":{"ref":"3"},"since":null,"typescript":"read"}]},{"class":"Http","interface":"cordis.http@1.0","methods":[{"async":true,"error":{"kind":"string"},"name":"route","params":[{"name":"spec","type":{"ref":"12"}},{"name":"handler","type":{"ref":"25"}}],"python":"route","result":{"kind":"u64"},"since":null,"typescript":"route"},{"async":true,"error":{"kind":"string"},"name":"unroute","params":[{"name":"id","type":{"kind":"u64"}}],"python":"unroute","result":{"kind":"unit"},"since":null,"typescript":"unroute"}]},{"class":"HttpHandler","interface":"cordis.http-handler@1.0","methods":[{"async":true,"error":{"kind":"string"},"name":"handle","params":[{"name":"request","type":{"ref":"15"}}],"python":"handle","result":{"ref":"16"},"since":null,"typescript":"handle"}]},{"class":"Pages","interface":"cordis.http-page@1.0","methods":[{"async":true,"error":{"kind":"string"},"name":"attributes","params":[{"name":"key","type":{"ref":"9"}}],"python":"attributes","result":{"ref":"23"},"since":null,"typescript":"attributes"}]}],"types":{"0":{"borrowed":false,"class":"CordisByteStreamStreamsByteStream","kind":"resource","service":"cordis.byte-stream@1.0"},"1":{"borrowed":true,"class":"CordisByteStreamStreamsByteStream","kind":"resource","service":"cordis.byte-stream@1.0"},"10":{"cases":[{"class":"BodyEmpty","name":"empty","type":null},{"class":"BodyInline","name":"inline","type":{"ref":"9"}},{"class":"BodyStream","name":"stream","type":{"ref":"8"}}],"class":"Body","kind":"variant"},"11":{"item":{"kind":"string"},"kind":"list"},"12":{"class":"RouteSpec","fields":[{"name":"prefix","python":"prefix","type":{"kind":"string"},"typescript":"prefix"},{"name":"methods","python":"methods","type":{"ref":"11"},"typescript":"methods"},{"name":"max-body-bytes","python":"max_body_bytes","type":{"ref":"6"},"typescript":"maxBodyBytes"},{"name":"timeout-ms","python":"timeout_ms","type":{"ref":"6"},"typescript":"timeoutMs"}],"kind":"record"},"13":{"items":[{"kind":"string"},{"kind":"string"}],"kind":"tuple"},"14":{"item":{"ref":"13"},"kind":"list"},"15":{"class":"HttpRequest","fields":[{"name":"method","python":"method","type":{"kind":"string"},"typescript":"method"},{"name":"path","python":"path","type":{"kind":"string"},"typescript":"path"},{"name":"prefix","python":"prefix","type":{"kind":"string"},"typescript":"prefix"},{"name":"query","python":"query","type":{"kind":"string"},"typescript":"query"},{"name":"headers","python":"headers","type":{"ref":"14"},"typescript":"headers"},{"name":"peer","python":"peer","type":{"kind":"string"},"typescript":"peer"},{"name":"body","python":"body","type":{"ref":"10"},"typescript":"body"}],"kind":"record"},"16":{"class":"HttpResponse","fields":[{"name":"status","python":"status","type":{"kind":"u16"},"typescript":"status"},{"name":"headers","python":"headers","type":{"ref":"14"},"typescript":"headers"},{"name":"body","python":"body","type":{"ref":"10"},"typescript":"body"}],"kind":"record"},"17":{"borrowed":false,"class":"HttpHandler","kind":"resource","service":"cordis.http-handler@1.0"},"18":{"borrowed":true,"class":"HttpHandler","kind":"resource","service":"cordis.http-handler@1.0"},"19":{"err":{"kind":"string"},"kind":"result","ok":{"ref":"16"}},"2":{"item":{"kind":"u8"},"kind":"list"},"20":{"err":{"kind":"string"},"kind":"result","ok":{"kind":"u64"}},"21":{"err":{"kind":"string"},"kind":"result","ok":{"kind":"unit"}},"22":{"class":"PageAttributes","fields":[{"name":"headers","python":"headers","type":{"ref":"14"},"typescript":"headers"},{"name":"peer","python":"peer","type":{"kind":"string"},"typescript":"peer"}],"kind":"record"},"23":{"item":{"ref":"22"},"kind":"option"},"24":{"err":{"kind":"string"},"kind":"result","ok":{"ref":"23"}},"25":{"borrowed":false,"class":"HttpHandler","kind":"resource","service":"cordis.http-handler@1.0"},"3":{"item":{"ref":"2"},"kind":"option"},"4":{"err":{"kind":"string"},"kind":"result","ok":{"ref":"3"}},"5":{"class":"CordisHttpHttpByteStream","item":{"ref":"0"},"kind":"alias"},"6":{"item":{"kind":"u64"},"kind":"option"},"7":{"borrowed":false,"class":"CordisByteStreamStreamsByteStream","kind":"resource","service":"cordis.byte-stream@1.0"},"8":{"class":"StreamBody","fields":[{"name":"length","python":"length","type":{"ref":"6"},"typescript":"length"},{"name":"source","python":"source","type":{"ref":"7"},"typescript":"source"}],"kind":"record"},"9":{"item":{"kind":"u8"},"kind":"list"}}};

export interface Streams {
}
export class StreamsClient {
    private readonly transport: Transport;
    constructor(transport: Transport) {
        this.transport = transport;
    }
}

export interface CordisByteStreamStreamsByteStream {
    read(maxBytes: number): Promise<Option<Uint8Array>>;
}
export class CordisByteStreamStreamsByteStreamClient {
    private readonly transport: Transport;
    constructor(transport: Transport) {
        this.transport = transport;
    }
    async read(maxBytes: number): Promise<Option<Uint8Array>> {
        return await invoke(this.transport, "cordis.byte-stream@1.0", 0, [maxBytes]) as Option<Uint8Array>;
    }
}

export interface Http {
    route(spec: RouteSpec, handler: HttpHandler): Promise<bigint>;
    unroute(id: bigint): Promise<void>;
}
export class HttpClient {
    private readonly transport: Transport;
    constructor(transport: Transport) {
        this.transport = transport;
    }
    async route(spec: RouteSpec, handler: HttpHandler): Promise<bigint> {
        return await invoke(this.transport, "cordis.http@1.0", 0, [spec, handler]) as bigint;
    }
    async unroute(id: bigint): Promise<void> {
        return await invoke(this.transport, "cordis.http@1.0", 1, [id]) as void;
    }
}

export interface HttpHandler {
    handle(request: HttpRequest): Promise<HttpResponse>;
}
export class HttpHandlerClient {
    private readonly transport: Transport;
    constructor(transport: Transport) {
        this.transport = transport;
    }
    async handle(request: HttpRequest): Promise<HttpResponse> {
        return await invoke(this.transport, "cordis.http-handler@1.0", 0, [request]) as HttpResponse;
    }
}

export interface Pages {
    attributes(key: Uint8Array): Promise<Option<PageAttributes>>;
}
export class PagesClient {
    private readonly transport: Transport;
    constructor(transport: Transport) {
        this.transport = transport;
    }
    async attributes(key: Uint8Array): Promise<Option<PageAttributes>> {
        return await invoke(this.transport, "cordis.http-page@1.0", 0, [key]) as Option<PageAttributes>;
    }
}

export type CordisHttpHttpByteStream = CordisByteStreamStreamsByteStream;
export interface StreamBody {
    length: Option<bigint>;
    source: CordisByteStreamStreamsByteStream;
}

export type Body = { kind: "empty" } | { kind: "inline"; value: Uint8Array } | { kind: "stream"; value: StreamBody };
export interface RouteSpec {
    prefix: string;
    methods: Array<string>;
    maxBodyBytes: Option<bigint>;
    timeoutMs: Option<bigint>;
}

export interface HttpRequest {
    method: string;
    path: string;
    prefix: string;
    query: string;
    headers: Array<[string, string]>;
    peer: string;
    body: Body;
}

export interface HttpResponse {
    status: number;
    headers: Array<[string, string]>;
    body: Body;
}

export interface PageAttributes {
    headers: Array<[string, string]>;
    peer: string;
}


const CLIENTS: Record<string, new (transport: Transport) => unknown> = {Streams: StreamsClient, CordisByteStreamStreamsByteStream: CordisByteStreamStreamsByteStreamClient, Http: HttpClient, HttpHandler: HttpHandlerClient, Pages: PagesClient};
const _runtime = bindRuntime(SCHEMA, CLIENTS);
export const { encodeValue, decodeValue, dispatch } = _runtime;
const { invoke } = _runtime;
