import {close, defineComponent} from "/_cordis/cordis-web.js";
import * as binding from "../page-counter.ts";

const interfaceName = "page-counter@1.0";
const realmName = "shared-browser-test";

/** DOC-15 §11.2：每页使用同一命名域，自身子组件通过真实服务读取本页状态。 */
export default defineComponent({
    name: "realm-counter",
    provides: [interfaceName],
    inject: {[interfaceName]: false},
    async apply(context, config) {
        if (config.consumer) {
            await consumer(context, config.owner);
            return;
        }
        let count = 0n;
        const effect = await context.effect(async scope => {
            const named = await scope.isolate(interfaceName, realmName);
            await scope.defer(() => named.close());
            await named.provide(interfaceName, {
                value() { return count; },
                increment() { return ++count; },
            }, binding);
            const launch = document.createElement("button");
            launch.textContent = "连接页面计数";
            launch.dataset.state = "available";
            const events = new AbortController();
            let pending = Promise.resolve();
            await scope.defer(async () => {
                events.abort();
                launch.remove();
                await pending;
            });
            launch.addEventListener("click", () => {
                launch.disabled = true;
                launch.dataset.state = "loading";
                pending = named.plugin("realm-counter", {consumer: true, owner: context.number.toString()}).then(() => {
                    launch.dataset.state = "ready";
                }, error => {
                    launch.dataset.state = "failed";
                    launch.textContent = error.message;
                });
            }, {signal: events.signal});
            document.querySelector("main#cordis-components").append(launch);
        }, "named-page-service").catch(error => {
            console.error("页面命名域效应失败", error);
            throw error;
        });
        effect.close();
    },
});

/** PR-6：页面控件与客户端归同一效应，在途动作完成后释放客户端。 */
async function consumer(context, owner) {
    const effect = await context.effect(async scope => {
        const service = await scope.get(interfaceName, binding, binding.PageCounterClient);
        const root = document.createElement("section");
        root.dataset.plugin = "realm-counter";
        root.dataset.owner = owner;
        root.dataset.realm = realmName;
        root.style.cssText = "max-width:760px;margin:24px auto;padding:24px;background:white;border-radius:16px";
        root.innerHTML = '<h2>页面命名域</h2><p>同一页面通过服务保存计数。</p><output aria-label="页面域计数" style="display:block;font-size:32px">0</output><button>读取页面计数</button> <button>增加页面计数</button><p role="status"></p>';
        const [read, increment] = root.querySelectorAll("button");
        const output = root.querySelector("output");
        const status = root.querySelector('[role="status"]');
        const events = new AbortController();
        const tasks = new Set();
        await scope.defer(async () => {
            events.abort();
            root.remove();
            await Promise.all(tasks);
            close(service);
        });
        const action = method => {
            read.disabled = increment.disabled = true;
            const task = (async () => {
                try {
                    output.value = (await service[method]()).toString();
                    status.textContent = "页面服务调用完成";
                } catch (error) {
                    status.textContent = `操作失败：${error.message}`;
                } finally {
                    read.disabled = increment.disabled = false;
                }
            })();
            tasks.add(task);
            void task.finally(() => tasks.delete(task));
        };
        read.addEventListener("click", () => action("value"), {signal: events.signal});
        increment.addEventListener("click", () => action("increment"), {signal: events.signal});
        document.querySelector("main#cordis-components").append(root);
        output.value = (await service.value()).toString();
    }, "named-page-counter");
    effect.close();
}
