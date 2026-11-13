import {close, defineComponent} from "/_cordis/cordis-web.js";
import * as binding from "../delay.ts";

/** AU-15：保留真实 get 返回的服务编号，原始故障帧沿此编号调用本页服务。 */
export default defineComponent({
    name: "flood-client",
    inject: {"delay@1.0": true},
    async apply(context) {
        const effect = await context.effect(async scope => {
            const service = await scope.get("delay@1.0", binding, binding.DelayClient);
            globalThis.cordisFloodObject = service.transport.reference.number.toString();
            await scope.defer(() => {
                delete globalThis.cordisFloodObject;
                close(service);
            });
        }, "flood-service-reference");
        effect.close();
    },
});
