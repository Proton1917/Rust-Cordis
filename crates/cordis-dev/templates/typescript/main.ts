import {run} from "./cordis_sdk/index.js";

await run("{{name}}", [{
    name: "{{name}}",
    async apply(context) {
        await context.log("{{name}} 已激活");
        const effect = await context.effect(async scope => {
            await scope.defer(async cleanup => {
                await cleanup.log("{{name}} 已清理");
            });
        }, "{{name}}");
        effect.close();
    },
}]);
