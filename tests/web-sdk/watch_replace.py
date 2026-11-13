import asyncio
import json
from pathlib import Path
import shutil
import subprocess


def prepare(case, plugins):
    shutil.copy2(case.tools / "libcordis_watch.dylib", plugins / "libcordis_watch.dylib")
    candidate = case.project / "watched/candidate"
    shutil.copytree(plugins / "store", candidate)
    shell = case.project / "shell"
    shell.mkdir()
    (shell / "index.html").write_text('<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>网页替换验收</title><style>body{font:16px/1.6 system-ui;margin:24px}header{padding:12px 0}</style><script src="/_cordis/session.js"></script><body><header id="application-shell">应用外壳</header><main id="cordis-components"></main><script type="module">import {connect} from "/_cordis/cordis-web.js";connect({token:CORDIS_SESSION.token});</script></body></html>')
    return '[instance.http.config]\nassets="shell"\nindex="index.html"\n[instance.watch]\nplugin="libcordis_watch.dylib"\ncomponent="watch"\ngrants={"cordis.manage@1"=["install","replace_batch"]}\nconfig={paths=["watched/candidate"],debounce_ms=300,configuration="cordis.toml"}\n'


def projection_snapshot(case):
    snapshot = case.status()
    rows = snapshot["instances"]
    projections = [item for item in rows if item.get("page_of") == "store"]
    if len(projections) == 2 and all(item["state"] == "active" for item in projections):
        assert all(item.get("page_of") is None for item in rows if item["name"] in {"store", "http", "watch", "notes"})
        return snapshot
    return None


def build_candidate(case):
    source = case.project / "candidate-source"
    output = case.project / "candidate-build"
    source.mkdir()
    (source / "main.js").write_text(f'import {{dashboard}} from {json.dumps(str(case.examples / "web-ui/dashboard.js"))};\nexport default dashboard("Watch");\n')
    with (case.project / "candidate-build.log").open("w") as log:
        subprocess.run(["node", str(Path(__file__).parent / "build-fixture.mjs"), str(case.options.modules), str(source), str(output)], check=True, stdout=log, stderr=subprocess.STDOUT)
    staged = case.project / "watched/candidate/web/index.js.next"
    shutil.copy2(output / "web/index.js", staged)
    staged.replace(staged.with_name("index.js"))


async def rejected_projection(case, snapshot):
    page = next(item for item in snapshot["instances"] if item.get("page_of") == "store")
    parent = next(item for item in snapshot["instances"] if item["name"] == "store")
    request = "explicit-page-target"
    ticket = await case.manage("replace_batch", [[[page["name"], parent["reference"]]], False, request])
    assert ticket["ok"], ticket
    def completed():
        result = json.loads((case.project / "state/batches.json").read_text()).get("admin")
        return result if result and result["id"] == request and result["state"] != "pending" else None
    outcome = await case.wait(completed, "显式页面目标批次拒绝")
    assert outcome["state"] == "rejected" and page["name"] in outcome["error"]["message"], outcome
    assert outcome["error"]["code"] == 4, outcome
    return outcome


async def run(case):
    before = await case.wait(lambda: projection_snapshot(case), "两页投影归属发布")
    origins = []
    for index, page in enumerate(case.pages):
        panel = page.locator('[data-plugin="kv-dashboard-rust"]')
        await panel.get_by_label("存储键", exact=True).fill(f"draft-{index + 1}")
        await panel.get_by_label("存储内容").fill(f"第{index + 1}页待保存草稿")
        origins.append(await page.evaluate('() => { globalThis.beforeDocument=document; globalThis.beforeShell=document.querySelector("#application-shell"); return performance.timeOrigin; }'))
    await asyncio.to_thread(build_candidate, case)
    def outcome():
        path = case.project / "state/batches.json"
        if not path.exists():
            return None
        result = json.loads(path.read_text()).get("watch")
        if result and result["state"] in {"rejected", "interrupted"}:
            raise AssertionError(result)
        return result if result and result["state"] == "succeeded" else None
    accepted = await case.wait(outcome, "watch 批次成功")
    for index, page in enumerate(case.pages):
        panel = page.locator('[data-plugin="kv-dashboard-watch"]')
        await panel.get_by_role("heading", name="Watch 数据便笺").wait_for()
        assert await panel.get_by_label("存储键", exact=True).input_value() == f"draft-{index + 1}"
        assert await panel.get_by_label("存储内容").input_value() == f"第{index + 1}页待保存草稿"
        assert await page.evaluate('document === beforeDocument && document.querySelector("#application-shell") === beforeShell')
        assert await page.evaluate("performance.timeOrigin") == origins[index]
        if index == 1:
            await page.set_viewport_size({"width": 390, "height": 844})
        await page.screenshot(path=str(case.project / f"watch-page-{index + 1}.png"), full_page=True)
    after = await case.wait(lambda: projection_snapshot(case), "替换后两页归属发布")
    rejected = await rejected_projection(case, after)
    return {"batch": accepted, "before": before, "after": after, "drafts": ["第1页待保存草稿", "第2页待保存草稿"], "documents_preserved": True, "shells_preserved": True, "explicit_projection_rejected": rejected}
