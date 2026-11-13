import {defineComponent, Frozen} from "/_cordis/cordis-web.js";
import {attach, panel, state, status} from "./panel.js";

export function notes() {
    return defineComponent({
        name: "web-notes", stateSchema: "web.notes", stateVersionMin: 1, stateVersionMax: 1,
        async apply(context) {
            const current = state(context, "web.notes", {text: "", count: 0});
            const frozen = new Frozen();
            await context.exportState("web.notes", 1, () => ({...current}), frozen);
            const ui = panel("web-notes", "页面笔记", "记下一段想法，保留这张页面里的编辑内容与计数。", `
                <div class="field"><label for="note">笔记内容</label><textarea id="note" placeholder="写下正在处理的事情…"></textarea></div>
                <div class="actions"><button data-action="clear">清空内容</button></div>
                <div class="counter"><div><div class="counter-label">本地计数</div><output aria-label="本地计数">0</output></div><button data-action="increment">计数加一</button></div>`);
            const input = ui.view.querySelector("textarea");
            const count = ui.view.querySelector("output");
            input.value = current.text; count.value = String(current.count);
            await attach(context, ui, ({signal, action}) => {
                input.addEventListener("input", () => action(() => {
                    frozen.check(); current.text = input.value; status(ui, "笔记已保存在当前页面");
                }), {signal});
                ui.view.querySelector('[data-action="clear"]').addEventListener("click", () => action(() => {
                    frozen.check(); current.text = ""; input.value = ""; status(ui, "内容已清空");
                }), {signal});
                ui.view.querySelector('[data-action="increment"]').addEventListener("click", () => action(() => {
                    frozen.check(); count.value = String(++current.count); status(ui, "计数已保存");
                }), {signal});
            });
        },
    });
}
