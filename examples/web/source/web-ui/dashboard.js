import {defineComponent, Frozen, close} from "/_cordis/cordis-web.js";
import * as binding from "./kv.ts";
import {attach, panel, state, status} from "./panel.js";

export function dashboard(kind) {
    return defineComponent({
        name: "kv-store/web", inject: {"kv@1.0": true}, stateSchema: "web.kv-dashboard", stateVersionMin: 1, stateVersionMax: 1,
        async apply(context) {
            const current = state(context, "web.kv-dashboard", {key: "greeting", value: "你好，Cordis", count: 0});
            const frozen = new Frozen();
            const store = await context.get("kv@1.0", binding, binding.KvClient);
            await context.exportState("web.kv-dashboard", 1, () => ({...current}), frozen);
            const ui = panel(`kv-dashboard-${kind.toLowerCase()}`, `${kind} 数据便笺`, "把一段文字放进共享存储，也可以按键读取已有内容。", `
                <div class="row"><div class="field"><label for="key">存储键</label><input id="key" autocomplete="off"></div><div class="field"><label for="value">存储内容</label><input id="value" autocomplete="off"></div></div>
                <div class="actions"><button class="primary" data-action="write">写入存储</button><button data-action="read">读取内容</button><button data-action="refresh">刷新键列表</button></div>
                <section class="records"><h3>已有存储键</h3><ul aria-label="存储键列表"></ul><p class="empty">正在读取存储键…</p></section>
                <div class="counter"><div><div class="counter-label">本地计数</div><output aria-label="本地计数">0</output></div><button data-action="increment">计数加一</button></div>`);
            const key = ui.view.querySelector("#key"), value = ui.view.querySelector("#value");
            const counter = ui.view.querySelector("output"), list = ui.view.querySelector("ul"), empty = ui.view.querySelector(".empty");
            key.value = current.key; value.value = current.value; counter.value = String(current.count);
            await attach(context, ui, async ({signal, action}) => {
                const read = async () => {
                    const result = await store.get(key.value);
                    frozen.check();
                    value.value = result === null ? "" : new TextDecoder().decode(result);
                    current.key = key.value; current.value = value.value;
                    status(ui, result === null ? "这个键为空" : "已读取存储内容");
                };
                const refresh = async () => {
                    const keys = await store.keys(); list.replaceChildren(); empty.hidden = keys.length > 0;
                    empty.textContent = "存储为空";
                    for (const name of keys) {
                        const row = document.createElement("li"), label = document.createElement("span"), select = document.createElement("button");
                        label.textContent = name; select.textContent = "读取"; select.setAttribute("aria-label", `读取 ${name}`);
                        select.addEventListener("click", () => action(async () => { frozen.check(); key.value = name; await read(); }), {signal});
                        row.append(label, select); list.append(row);
                    }
                };
                for (const field of [key, value]) field.addEventListener("input", () => action(() => {
                    frozen.check(); current.key = key.value; current.value = value.value;
                }), {signal});
                ui.view.querySelector('[data-action="write"]').addEventListener("click", () => action(async () => {
                    frozen.check(); await store.put(key.value, new TextEncoder().encode(value.value)); await store.flush();
                    frozen.check();
                    current.key = key.value; current.value = value.value; await refresh(); status(ui, "内容已写入存储");
                }), {signal});
                ui.view.querySelector('[data-action="read"]').addEventListener("click", () => action(async () => { frozen.check(); await read(); }), {signal});
                ui.view.querySelector('[data-action="refresh"]').addEventListener("click", () => action(refresh), {signal});
                ui.view.querySelector('[data-action="increment"]').addEventListener("click", () => action(() => {
                    frozen.check(); counter.value = String(++current.count); status(ui, "本地计数已保存");
                }), {signal});
                await refresh(); status(ui, "存储已连接");
            }, () => close(store));
        },
    });
}
