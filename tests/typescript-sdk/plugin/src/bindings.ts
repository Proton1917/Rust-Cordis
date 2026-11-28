// DOC-15 §7、ADR-0039：绑定以显式 Transport 管理调用与资源引用。
import { Tagged, Some, bindRuntime, type Option, type ResultValue, type Transport, type Schema } from "cordis-sdk";
export { Tagged, Some, type Option, type ResultValue, type Transport };

export const SCHEMA: Schema = {"package":"cordis:kv@1.0.0","services":[{"class":"Kv","interface":"kv@1.0","methods":[{"async":false,"error":{"kind":"string"},"name":"get","params":[{"name":"key","type":{"kind":"string"}}],"python":"get","result":{"ref":"4"},"since":null,"typescript":"get"},{"async":false,"error":{"kind":"string"},"name":"put","params":[{"name":"key","type":{"kind":"string"}},{"name":"value","type":{"ref":"0"}}],"python":"put","result":{"kind":"unit"},"since":null,"typescript":"put"},{"async":false,"error":{"kind":"string"},"name":"keys","params":[],"python":"keys","result":{"ref":"7"},"since":null,"typescript":"keys"},{"async":true,"error":{"kind":"string"},"name":"flush","params":[],"python":"flush","result":{"kind":"unit"},"since":null,"typescript":"flush"}]}],"types":{"0":{"item":{"kind":"u8"},"kind":"list"},"1":{"item":{"ref":"0"},"key":{"kind":"string"},"kind":"map"},"2":{"class":"Snapshot","fields":[{"name":"entries","python":"entries","type":{"ref":"1"},"typescript":"entries"}],"kind":"record"},"3":{"class":"Changed","fields":[{"name":"key","python":"key","type":{"kind":"string"},"typescript":"key"}],"kind":"record"},"4":{"item":{"ref":"0"},"kind":"option"},"5":{"err":{"kind":"string"},"kind":"result","ok":{"ref":"4"}},"6":{"err":{"kind":"string"},"kind":"result","ok":{"kind":"unit"}},"7":{"item":{"kind":"string"},"kind":"list"},"8":{"err":{"kind":"string"},"kind":"result","ok":{"ref":"7"}}}};

export interface Kv {
    get(key: string): Option<Uint8Array>;
    put(key: string, value: Uint8Array): void;
    keys(): Array<string>;
    flush(): Promise<void>;
}
export class KvClient {
    private readonly transport: Transport;
    constructor(transport: Transport) {
        this.transport = transport;
    }
    async get(key: string): Promise<Option<Uint8Array>> {
        return await invoke(this.transport, "kv@1.0", 0, [key]) as Option<Uint8Array>;
    }
    async put(key: string, value: Uint8Array): Promise<void> {
        return await invoke(this.transport, "kv@1.0", 1, [key, value]) as void;
    }
    async keys(): Promise<Array<string>> {
        return await invoke(this.transport, "kv@1.0", 2, []) as Array<string>;
    }
    async flush(): Promise<void> {
        return await invoke(this.transport, "kv@1.0", 3, []) as void;
    }
}

export interface Snapshot {
    entries: Map<string, Uint8Array>;
}

export interface Changed {
    key: string;
}


const CLIENTS: Record<string, new (transport: Transport) => unknown> = {Kv: KvClient};
const _runtime = bindRuntime(SCHEMA, CLIENTS);
export const { encodeValue, decodeValue, dispatch } = _runtime;
const { invoke } = _runtime;
