import component from './base.js';
export default {...component, async apply() { throw new Error('候选页面激活失败'); }};
