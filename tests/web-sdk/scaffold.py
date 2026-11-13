import asyncio
from pathlib import Path
import shutil


async def command(case, label, *arguments):
    process = await asyncio.create_subprocess_exec(
        *map(str, arguments), cwd=case.project,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT,
    )
    output, _ = await process.communicate()
    (case.project / f"{label}.log").write_bytes(output)
    if process.returncode:
        raise RuntimeError(f"{label}: {output.decode(errors='replace')}")


async def generate(case):
    root = Path(__file__).resolve().parents[2]
    generated = case.project / "generated"
    dev = case.tools / "cordis-dev"
    await command(case, "scaffold-project", dev, "new", generated, "--sdk-path", root / "crates/cordis-sdk")
    await command(case, "scaffold-web", dev, "new-plugin", "page-counter", "--web", "--project", generated)
    return generated / "plugins/page-counter"


async def counter(page, expected):
    panel = page.locator('[data-plugin="page-counter"]')
    output = panel.get_by_label("计数", exact=True)
    await output.wait_for()
    assert await output.inner_text() == str(expected)
    return panel


async def run(case):
    """DOC-15 §12.1：生成的真实模块动态加入两页，在替换和释放时保留各页生命周期。"""
    source = await generate(case)
    installed = await case.manage("install", [str(source)])
    assert installed["ok"], installed
    added = await case.manage("add", ["page-counter", installed["value"], None, {}])
    assert added["ok"], added
    origins = [await page.evaluate("performance.timeOrigin") for page in case.pages]
    for index, page in enumerate(case.pages):
        panel = await counter(page, 0)
        for _ in range(index + 1):
            await panel.get_by_role("button", name="计数加一", exact=True).click()
        await counter(page, index + 1)

    candidate = case.project / "sources/scaffold-next"
    shutil.copytree(source, candidate)
    entry = candidate / "web/index.js"
    entry.write_text(entry.read_text().replace("记录当前页面中的计数。", "记录本页的操作次数。"))
    await case.replace("page-counter", candidate)
    for index, page in enumerate(case.pages):
        await page.get_by_text("记录本页的操作次数。", exact=True).wait_for()
        await counter(page, index + 1)
        assert await page.evaluate("performance.timeOrigin") == origins[index]
    await case.pages[0].screenshot(path=str(case.project / "scaffold-desktop.png"), full_page=True)
    await case.pages[1].set_viewport_size({"width": 390, "height": 844})
    await case.pages[1].screenshot(path=str(case.project / "scaffold-mobile.png"), full_page=True)

    removed = await case.manage("remove", ["page-counter"])
    assert removed["ok"], removed
    for page in case.pages:
        await page.locator('[data-plugin="page-counter"]').wait_for(state="detached")
    await case.wait(
        lambda: all(item["name"] != "page-counter" and not item["name"].endswith("-page-counter")
                    for item in case.status()["instances"]),
        "生成插件及页面子实例回收",
    )
    session = await case.session()
    response = await case.context.request.get(
        case.url + "_cordis/manifest.json", headers={"Authorization": f"Bearer {session['token']}"},
    )
    assert response.ok
    assert all(entry["name"] != "page-counter" for entry in await response.json())
    return {"generated_plugin": "page-counter", "counts": [1, 2], "state_inherited": True,
            "page_reload": False, "children_reclaimed": True, "registration_removed": True}
