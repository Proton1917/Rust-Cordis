import {close, defineComponent} from "/_cordis/cordis-web.js";
import * as text from "../text-services.ts";
import * as kv from "../../../../examples/web/source/web-ui/kv.ts";
import {theme} from "../../../../examples/web/source/web-ui/theme.js";

/** DOC-15 AU-14：同一页面同时依赖三个真实后端，效应随提供方撤回。 */
export default defineComponent({
    name: "service-dependencies",
    inject: {"kv@1.0": true, "text-normalizer@1.0": true, "text-length@1.0": true},
    async apply(context) {
        const effect = await context.effect(async scope => {
            const store = await scope.get("kv@1.0", kv, kv.KvClient);
            const normalizer = await scope.get("text-normalizer@1.0", text, text.TextNormalizerClient);
            const measure = await scope.get("text-length@1.0", text, text.TextLengthClient);
            const root = document.createElement("section");
            root.dataset.plugin = "service-dependencies";
            root.dataset.context = context.number.toString();
            const shadow = root.attachShadow({mode: "open"});
            shadow.innerHTML = `<style>${theme} output {font-size:20px;overflow-wrap:anywhere;min-width:0;text-align:right}</style><article><header><div><p class="eyebrow">文本工作台</p><h2>规范化与存储</h2><p class="description">整理字符形式，计算字符数量，并保存文本。</p></div></header><div class="field"><label for="value">待处理文本</label><input id="value" value="Ｃｏｒｄｉｓ🌍"></div><div class="actions"><button class="primary">处理并保存</button></div><div class="counter"><span class="counter-label">存储结果</span><output aria-label="跨语言处理结果"></output></div><p class="status" role="status"></p></article>`;
            const input = shadow.querySelector("input");
            const button = shadow.querySelector("button");
            const output = shadow.querySelector("output");
            const status = shadow.querySelector('[role="status"]');
            const events = new AbortController();
            const tasks = new Set();
            await scope.defer(async () => {
                events.abort();
                root.remove();
                await Promise.all(tasks);
                close(measure);
                close(normalizer);
                close(store);
            });
            button.addEventListener("click", () => {
                button.disabled = true;
                const task = (async () => {
                    try {
                        const normalized = await normalizer.normalize(input.value);
                        const length = await measure.measure(normalized);
                        const key = `page-${context.number}`;
                        await store.put(key, new TextEncoder().encode(normalized));
                        const stored = new TextDecoder().decode(await store.get(key));
                        output.value = `${stored} · ${length} 字符`;
                        status.textContent = "文本已保存并读取";
                    } catch (error) {
                        status.dataset.error = "true";
                        status.textContent = `操作失败：${error.message}`;
                    } finally {
                        button.disabled = false;
                    }
                })();
                tasks.add(task);
                void task.finally(() => tasks.delete(task));
            }, {signal: events.signal});
            document.querySelector("main#cordis-components").append(root);
        }, "cross-language-page");
        effect.close();
    },
});
