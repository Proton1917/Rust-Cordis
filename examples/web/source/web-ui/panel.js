import {CordisError} from "/_cordis/cordis-web.js";
import {theme} from "./theme.js";

export function state(context, schema, defaults) {
    if (context.inherited === null) return {...defaults};
    const inherited = context.inherited;
    if (inherited.get("schema") !== schema || inherited.get("version") !== 1) {
        throw new CordisError(2, "页面状态需要匹配组件版本");
    }
    return {...defaults, ...Object.fromEntries(inherited.get("data"))};
}

export function panel(id, title, description, content) {
    const root = document.createElement("section");
    root.dataset.plugin = id;
    const view = root.attachShadow({mode: "open"});
    view.innerHTML = `<style>${theme}</style><article><header><div><p class="eyebrow">CORDIS WORKSPACE</p><h2>${title}</h2><p class="description">${description}</p></div></header>${content}<p class="status" role="status" aria-live="polite"></p></article>`;
    return {root, view, status: view.querySelector(".status")};
}

export function status(panel, text, error = false) {
    panel.status.textContent = text;
    panel.status.dataset.error = String(error);
}

/** DOC-15 §11.2：DOM、事件监听和在途界面操作归同一个组件效应。 */
export async function attach(context, panel, setup, cleanup = () => {}) {
    const tasks = new Set();
    const events = new AbortController();
    const action = operation => {
        const pending = (async () => {
            try { await operation(); }
            catch (error) { status(panel, error instanceof Error ? error.message : String(error), true); }
        })();
        tasks.add(pending);
        void pending.finally(() => tasks.delete(pending));
    };
    const effect = await context.effect(async scope => {
        await scope.defer(async () => {
            events.abort(); panel.root.remove();
            await Promise.all(tasks); await cleanup();
        });
        document.querySelector("main#cordis-components").append(panel.root);
        await setup({signal: events.signal, action});
    }, "web-panel");
    effect.close();
}
