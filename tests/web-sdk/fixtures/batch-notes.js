import component from "./base.js";

/** DOC-15 §11：同一候选同时展示版本替换与实际配置驱动的页面重建。 */
export default {...component, async apply(context, config) {
    await component.apply(context, config);
    document.querySelector('[data-plugin="web-notes"]').shadowRoot.querySelector("h2").textContent = config.title ?? "批次工作笔记";
}};
