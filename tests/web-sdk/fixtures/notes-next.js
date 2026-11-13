import component from './base.js';
export default {...component, async apply(context, config) {
    await component.apply(context, config);
    document.querySelector('[data-plugin="web-notes"]').shadowRoot.querySelector('h2').textContent = '工作笔记';
}};
