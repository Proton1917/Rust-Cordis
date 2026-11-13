import component from './base.js';
export default {...component, async apply(context, config) {
    await component.apply(context, config);
    document.querySelector('[data-plugin="kv-dashboard-rust"]').shadowRoot.querySelector('h2').textContent = '项目数据便笺';
}};
