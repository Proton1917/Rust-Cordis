import {defineComponent, Frozen} from "/_cordis/cordis-web.js";

export default defineComponent({
    name: "{{component}}",
    stateSchema: "{{name}}.page",
    stateVersionMin: 1,
    stateVersionMax: 1,
    async apply(context) {
        let count = context.inherited?.get("data").get("count") ?? 0;
        const frozen = new Frozen();
        await context.exportState("{{name}}.page", 1, () => ({count}), frozen);
        const panel = document.createElement("section");
        panel.dataset.plugin = "{{name}}";
        const view = panel.attachShadow({mode: "open"});
        const stylesheet = new URL("./style.css", import.meta.url);
        view.innerHTML = `<link rel="stylesheet" href="${stylesheet}">
            <article><h2>{{name}}</h2><p>记录当前页面中的计数。</p>
            <output aria-label="计数" aria-live="polite"></output>
            <button type="button">计数加一</button><p role="status"></p></article>`;
        const output = view.querySelector("output");
        output.value = String(count);
        const events = new AbortController();
        const effect = await context.effect(async scope => {
            await scope.defer(() => { events.abort(); panel.remove(); });
            view.querySelector("button").addEventListener("click", () => {
                if (frozen.frozen) {
                    view.querySelector('[role="status"]').textContent = "正在保存状态，请稍后操作。";
                    return;
                }
                frozen.check();
                output.value = String(++count);
                view.querySelector('[role="status"]').textContent = "计数已保存。";
            }, {signal: events.signal});
            document.querySelector("main#cordis-components").append(panel);
        }, "page");
        effect.close();
    },
});
