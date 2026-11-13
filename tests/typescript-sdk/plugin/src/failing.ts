/** DOC-11 SW-6：真实 apply 错误触发旧提供方解冻和状态回退。 */
import {CordisError, run} from "cordis-sdk";

await run("kv", [{name: "kv-store", provides: ["kv@1.0"], stateSchema: "kv.snapshot",
    stateVersionMin: 1, stateVersionMax: 1,
    apply() { throw new CordisError(1001, "候选激活失败"); },
}]);
