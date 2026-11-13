// Open Claude UI 02d021f：复用主题颜色、40px 按钮和焦点轮廓，作用域限定于组件 ShadowRoot。
export const theme = `
:host { display:block; color:#151515; font:15px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC",sans-serif; --surface:#fff; --muted:#525252; --border:rgba(21,21,21,.1); --soft:#f5f5f3; --clay:#d97757; --clay-hover:#cc5a36; }
* { box-sizing:border-box; }
article { background:var(--surface); border:1px solid var(--border); border-radius:16px; padding:28px; box-shadow:0 2px 8px rgba(0,0,0,.035); max-width:760px; margin:0 auto; }
header { display:flex; justify-content:space-between; align-items:flex-start; gap:16px; margin-bottom:24px; }
.eyebrow { color:var(--clay); font-size:12px; font-weight:650; letter-spacing:.08em; margin:0 0 6px; }
h2 { font-size:24px; line-height:1.3; letter-spacing:-.025em; margin:0; font-weight:650; }
.description { color:var(--muted); margin:8px 0 0; max-width:50ch; font-size:14px; }
.field { display:grid; gap:7px; margin:0 0 16px; }
label { font-size:13px; font-weight:600; }
input,textarea { font:inherit; color:inherit; width:100%; border:1px solid var(--border); border-radius:10px; background:var(--surface); padding:10px 12px; outline:none; }
textarea { min-height:126px; resize:vertical; line-height:1.65; }
input:focus-visible,textarea:focus-visible,button:focus-visible { outline:2px solid var(--clay); outline-offset:3px; }
.row { display:grid; grid-template-columns:minmax(0,1fr) minmax(0,1.6fr); gap:16px; }
.actions { display:flex; flex-wrap:wrap; gap:10px; align-items:center; }
button { min-height:40px; border:0; border-radius:12px; padding:8px 18px; background:var(--soft); color:inherit; cursor:pointer; font-family:inherit; font-size:14px; font-weight:600; line-height:1.3; transition:background-color 150ms,transform 150ms; }
button:hover { background:color-mix(in srgb,var(--soft) 80%,var(--muted)); }
button:active { transform:scale(.98); }
button.primary { background:#b85e40; color:white; }
button.primary:hover { background:#9e4b31; }
button:disabled { opacity:.45; cursor:wait; transform:none; }
.status { min-height:24px; margin:14px 0 0; color:var(--muted); font-size:13px; overflow-wrap:anywhere; }
.status[data-error=true] { color:#b83d2f; }
.counter { display:flex; align-items:center; justify-content:space-between; gap:18px; margin-top:24px; padding-top:22px; border-top:1px solid var(--border); }
.counter-label { font-size:13px; color:var(--muted); }
output { display:block; font-size:30px; line-height:1.3; font-weight:650; font-variant-numeric:tabular-nums; }
.records { margin:22px 0 0; border-top:1px solid var(--border); }
.records h3 { margin:18px 0 12px; font-size:14px; }
ul { list-style:none; padding:0; margin:0; display:grid; gap:8px; }
li { display:flex; gap:16px; justify-content:space-between; align-items:center; padding:10px 12px; background:var(--soft); border-radius:8px; }
li span { overflow-wrap:anywhere; min-width:0; }
li button { min-height:32px; padding:6px 10px; background:var(--surface); font-size:12px; flex-shrink:0; }
.empty { color:var(--muted); font-size:13px; padding:8px 0; }
@media (max-width:520px) { article { padding:20px; border-radius:12px; } h2 { font-size:21px; } .row { grid-template-columns:1fr; gap:0; } .actions { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); } .actions button { white-space:nowrap; } .actions button:last-child:nth-child(odd) { grid-column:1/-1; } header { gap:8px; } }
@media (prefers-color-scheme:dark) { :host { color:#f2f1ec; --surface:#1a1a19; --muted:#b4b4ae; --border:rgba(255,255,255,.12); --soft:#272725; --clay:#d97757; --clay-hover:#e08264; } .status[data-error=true] { color:#ff9987; } }
@media (prefers-reduced-motion:reduce) { button { transition:none; } button:active { transform:none; } }
`;
