import {CordisError, defineComponent} from "/_cordis/cordis-web.js";
import * as binding from "../delay.ts";

/** CP-3/4、AU-15：真实服务调用保持在途，页面效应结束时回收等待者。 */
export default defineComponent({
    name: "flood-provider",
    provides: ["delay@1.0"],
    async apply(context) {
        const waiting = new Set();
        const metrics = {started: 0, active: 0, peak: 0};
        globalThis.cordisFloodMetrics = metrics;
        const effect = await context.effect(async scope => {
            await scope.defer(() => {
                for (const reject of waiting) reject(new CordisError(4, "页面等待服务已结束"));
                waiting.clear();
            });
            await scope.provide("delay@1.0", {
                async pause() {
                    metrics.started++;
                    metrics.active++;
                    metrics.peak = Math.max(metrics.peak, metrics.active);
                    let pending;
                    try {
                        return await new Promise((_resolve, reject) => {
                            pending = reject;
                            waiting.add(reject);
                        });
                    } finally {
                        metrics.active--;
                        waiting.delete(pending);
                    }
                },
            }, binding);
        }, "flood-waiters");
        effect.close();
    },
});
