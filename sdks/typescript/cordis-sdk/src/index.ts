/** DOC-15 §10：TypeScript 进程插件的作者接口。 */
export { Context, Effect, Frozen, defineComponent, type Component } from "./context.js";
export { BindingTransport, Remote, close } from "./bindings.js";
export { CordisError, type Binding, type Transport } from "./types.js";
export { run } from "./node.js";
export { currentSignal, withSignal } from "./runtime.js";
export { DEFAULT_LIMITS, type Limits } from "./wire.js";
