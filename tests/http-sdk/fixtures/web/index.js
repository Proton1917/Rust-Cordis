import {caller, close, defineComponent} from "/_cordis/cordis-web.js";
import * as identityBinding from "./identity.js";
import * as secretBinding from "./secrets.js";

export default defineComponent({
    name: "identity-provider/web",
    inject: {"test.identity@1.0": true, "cordis.secrets@1.0": true},
    async apply(context) {
        const effect = await context.effect(async scope => {
        const service = await context.get("test.identity@1.0", identityBinding, identityBinding.IdentityClient);
        await scope.defer(() => close(service));
        const report = JSON.parse(await service.inspect());
        const trusted = caller(new Map([["cordis.caller", new Map(Object.entries(report.metadata["cordis.caller"]))]]));
        let forgedCode;
        try {
            await context.intercept("test.identity@1.0", new Map([["cordis.page", new Map([["forwarder", "forged"], ["key", [1, 2, 3]]])]]));
        } catch (error) { forgedCode = error.code; }
        const secrets = await context.get("cordis.secrets@1.0", secretBinding, secretBinding.SecretsClient);
        await scope.defer(() => close(secrets));
        let secretCode;
        try { await secrets.get("ALPHA"); } catch (error) { secretCode = error.code; }
        if (forgedCode !== 9 || secretCode !== 9 || !trusted?.page_of || trusted.instance !== undefined) {
            throw new Error("页面身份或密钥权限结果错误");
        }
            const panel = document.createElement("section");
            panel.id = "identity-report";
            panel.dataset.report = JSON.stringify({...report, forgedCode, secretCode, trusted});
            const heading = document.createElement("h1");
            heading.textContent = "调用来源";
            const text = document.createElement("p");
            text.textContent = "用户信息已读取。";
            panel.append(heading, text);
            document.querySelector("#cordis-components").append(panel);
            await scope.defer(() => panel.remove());
        }, "identity-view", true);
        await effect.settled();
    }
});
