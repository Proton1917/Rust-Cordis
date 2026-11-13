import json
from pathlib import Path
import shutil


def panel(page, name):
    return page.locator(f'[data-plugin="{name}"]')


def active_reference(snapshot, name, reference):
    return any(
        item["name"] == name and item["state"] == "active" and item["reference"] == reference
        for item in snapshot["instances"]
    )


async def install(case, source):
    reply = await case.manage("install", [str(source)])
    assert reply["ok"], reply
    return reply["value"]


async def replace_batch(case, replacements, request_id, expected):
    """ADR-0031：受理票据匹配请求，最终结果读取核心持久化的权威记录。"""
    ticket = await case.manage("replace_batch", [replacements, False, request_id])
    assert ticket["ok"] and ticket["value"]["id"] == request_id, ticket

    def completed():
        path = case.project / "state/batches.json"
        records = json.loads(path.read_text())
        record = records["admin"]
        return record if record["id"] == request_id and record["state"] != "pending" else None

    outcome = await case.wait(completed, f"{request_id} 权威结果完成")
    assert outcome["state"] == expected, outcome
    if expected == "rejected":
        assert "候选页面激活失败" in json.dumps(outcome["error"], ensure_ascii=False), outcome
        detail = outcome["error"]["detail"]
        assert isinstance(detail, dict) and detail.get("committed") is False, outcome
    result = {"ticket": ticket, "outcome": outcome}
    case.save(request_id + ".json", result)
    return result


async def read_shared(page):
    """AU-7：改写输入后经真实服务重新读取，核对后端恢复的值。"""
    store = panel(page, "kv-dashboard-rust")
    await store.get_by_label("存储键", exact=True).fill("batch-shared")
    await store.get_by_label("存储内容", exact=True).fill("等待后端读取")
    await store.get_by_role("button", name="读取内容", exact=True).click()
    field = store.get_by_label("存储内容", exact=True)
    await page.wait_for_function(
        "field => field.value === '批次共享内容'", arg=await field.element_handle(),
    )
    assert await field.input_value() == "批次共享内容"


async def verify_pages(case, origins, notes_title, store_title, stage):
    """AU-7：两页分别保留笔记与计数，成功和恢复路径均保持页面文档。"""
    for index, page in enumerate(case.pages):
        notes = panel(page, "web-notes")
        store = panel(page, "kv-dashboard-rust")
        await notes.get_by_role("heading", name=notes_title, exact=True).wait_for()
        await store.get_by_role("heading", name=store_title, exact=True).wait_for()
        assert await notes.get_by_label("笔记内容").input_value() == f"第{index + 1}页批次笔记"
        assert await notes.get_by_label("本地计数", exact=True).inner_text() == str(index + 1)
        assert await store.get_by_label("本地计数", exact=True).inner_text() == str(2 - index)
        assert await page.evaluate("performance.timeOrigin") == origins[index]
        await read_shared(page)
        await page.screenshot(path=str(case.project / f"batch-{stage}-{index + 1}.png"), full_page=True)


async def initialize(case):
    """M20：两页的可观察状态独立，后端存储经生成 KV Client 共享。"""
    await case.wait(
        lambda: all(item["state"] == "active" for item in case.status()["instances"]),
        "初始网页与后端实例进入 Active",
    )
    for index, page in enumerate(case.pages):
        notes = panel(page, "web-notes")
        store = panel(page, "kv-dashboard-rust")
        await store.get_by_role("heading", name="Rust 数据便笺", exact=True).wait_for()
        await notes.get_by_label("笔记内容").fill(f"第{index + 1}页批次笔记")
        for _ in range(index + 1):
            await notes.get_by_role("button", name="计数加一", exact=True).click()
        for _ in range(2 - index):
            await store.get_by_role("button", name="计数加一", exact=True).click()
    store = panel(case.pages[0], "kv-dashboard-rust")
    await store.get_by_label("存储键", exact=True).fill("batch-shared")
    await store.get_by_label("存储内容", exact=True).fill("批次共享内容")
    await store.get_by_role("button", name="写入存储", exact=True).click()
    await store.get_by_role("status").filter(has_text="内容已写入存储").wait_for()
    return [await page.evaluate("performance.timeOrigin") for page in case.pages]


async def configure_and_remove(case, origins):
    """DOC-15 §3.2/§11：配置驱动页面标题，移除操作回收对应资源与后端。"""
    configured = await case.manage("configure", ["notes", {"title": "配置后的笔记"}])
    assert configured["ok"], configured
    await verify_pages(case, origins, "配置后的笔记", "项目数据便笺", "configured")
    removed = await case.manage("remove", ["notes"])
    assert removed["ok"], removed
    await case.wait(
        lambda: all(item["name"] != "notes" and not item["name"].endswith("-notes") for item in case.status()["instances"]),
        "笔记 Owner 与两页子实例回收",
    )
    for page in case.pages:
        await panel(page, "web-notes").wait_for(state="detached")
        await read_shared(page)
    removed_store = await case.manage("remove", ["store"])
    assert removed_store["ok"], removed_store
    await case.wait(
        lambda: all(item["name"] != "store" and not item["name"].endswith("-store") for item in case.status()["instances"]),
        "存储后端及其网页子实例回收",
    )
    for index, page in enumerate(case.pages):
        await panel(page, "kv-dashboard-rust").wait_for(state="detached")
        assert await page.evaluate("performance.timeOrigin") == origins[index]
    session = await case.session()
    response = await case.context.request.get(case.url + "_cordis/manifest.json", headers={"Authorization": f'Bearer {session["token"]}'})
    assert response.ok
    assert await response.json() == []
    return {"configure": configured, "remove_notes": removed, "remove_store": removed_store}


async def run(case):
    """ADR-0031、AU-7：失败批次整体恢复，成功批次发布两种网页组件及后端。"""
    origins = await initialize(case)
    original = {item["name"]: item["reference"] for item in case.status()["instances"] if item["name"] in ("notes", "store")}
    notes = case.candidate(case.examples / "plugins/web-notes", "batch-notes", "notes-next.js")
    shutil.copy2(Path(__file__).with_name("fixtures") / "batch-notes.js", notes / "web/index.js")
    store = case.candidate(case.examples / "plugins/kv-dashboard-rust", "batch-store", "kv-next.js")
    failed = case.candidate(case.examples / "plugins/kv-dashboard-rust", "batch-rejected-store", "rejected.js")
    for candidate in (store, failed):
        shutil.copy2(case.tools / "libkv_mem.dylib", candidate / "libkv_mem.dylib")
    candidates = {"notes": await install(case, notes), "store": await install(case, store), "failed": await install(case, failed)}
    rejected = await replace_batch(case, [["notes", candidates["notes"]], ["store", candidates["failed"]]], "web-batch-rejected", "rejected")
    await case.wait(lambda: all(active_reference(case.status(), name, reference) for name, reference in original.items()), "原笔记与后端来源恢复")
    await verify_pages(case, origins, "页面笔记", "Rust 数据便笺", "restored")
    accepted = await replace_batch(case, [["notes", candidates["notes"]], ["store", candidates["store"]]], "web-batch-accepted", "succeeded")
    await case.wait(lambda: all(active_reference(case.status(), name, candidates[name]) for name in ("notes", "store")), "两项候选来源发布")
    await verify_pages(case, origins, "批次工作笔记", "项目数据便笺", "accepted")
    changes = await configure_and_remove(case, origins)
    result = {"rejected": rejected, "accepted": accepted, "changes": changes, "origins": origins}
    case.save("web-batch.json", result)
    return result
