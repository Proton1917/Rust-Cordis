from pathlib import Path
import shutil
import subprocess


def prepare(case, plugins):
    fixtures = case.options.modules.resolve() / "fixtures"
    workspace = case.options.modules.resolve().parents[1]
    python = plugins / "python-service"
    case.manifest(python, "web-python-service", ["conda", "run", "--no-capture-output", "-n", "base", "python", "-B", "main.py"])
    shutil.copy2(fixtures / "python-service/main.py", python / "main.py")
    shutil.copy2(fixtures / "text_services.py", python / "text_services.py")
    shutil.copytree(workspace / "sdks/python/cordis_sdk", python / "cordis_sdk", ignore=shutil.ignore_patterns("__pycache__"))
    typescript = plugins / "typescript-service"
    case.manifest(typescript, "web-typescript-service", ["node", "main.cjs"])
    with (case.project / "typescript-service-build.log").open("w") as log:
        subprocess.run([
            "node", str(Path(__file__).parent / "build-node.mjs"), str(case.options.modules.resolve()),
            str(fixtures / "typescript-service"), str(typescript),
        ], check=True, stdout=log, stderr=subprocess.STDOUT)
    shutil.copy2(case.tools / "libkv_mem.dylib", plugins / "libkv_mem.dylib")
    case.fixture("service-dependencies", plugins)
    return ('[instance.rust-service]\nplugin="kv"\ncomponent="kv-store"\n'
            '[instance.python-service]\nplugin="web-python-service"\ncomponent="normalizer"\n'
            '[instance.typescript-service]\nplugin="web-typescript-service"\ncomponent="length"\n'
            '[instance.service-dependencies]\nplugin="service-dependencies"\n')


async def process(page, value="Ｃｏｒｄｉｓ🌍", expected="Cordis🌍 · 7 字符"):
    panel = page.locator('[data-plugin="service-dependencies"]')
    await panel.get_by_label("待处理文本", exact=True).fill(value)
    button = panel.get_by_role("button", name="处理并保存", exact=True)
    await button.click()
    await page.wait_for_function("button => !button.disabled", arg=await button.element_handle())
    assert await panel.get_by_label("跨语言处理结果", exact=True).inner_text() == expected
    assert await panel.locator('p[role="status"]').inner_text() == "文本已保存并读取"


def pages(case):
    return [item for item in case.status()["instances"] if item["name"].startswith("web-") and item["name"].endswith("-service-dependencies")]


async def run(case):
    for page in case.pages:
        await process(page)
    origins = [await page.evaluate("performance.timeOrigin") for page in case.pages]
    initial = pages(case)
    assert len(initial) == 2
    case.save("dependencies-initial.json", case.status())
    await case.pages[0].screenshot(path=str(case.project / "dependencies-active.png"), full_page=True)
    changes = []
    for instance, filename in [
        ("rust-service", "libkv_mem.dylib"),
        ("python-service", "python-service"),
        ("typescript-service", "typescript-service"),
    ]:
        installed = await case.manage("install", [str(case.project / "plugins" / filename)])
        assert installed["ok"], installed
        removed = await case.manage("remove", [instance])
        assert removed["ok"], removed
        await case.wait(lambda: len(pages(case)) == 2 and all(item["state"] == "pending" for item in pages(case)), f"{instance} 撤回后两页挂起")
        for page in case.pages:
            await page.locator('[data-plugin="service-dependencies"]').wait_for(state="detached")
        pending = case.status()
        if instance == "rust-service":
            await case.pages[0].screenshot(path=str(case.project / "dependencies-pending.png"), full_page=True)
        added = await case.manage("add", [instance, installed["value"], None, {}])
        assert added["ok"], added
        await case.wait(lambda: len(pages(case)) == 2 and all(item["state"] == "active" for item in pages(case)), f"{instance} 插回后两页恢复")
        for index, page in enumerate(case.pages):
            await process(page)
            assert await page.evaluate("performance.timeOrigin") == origins[index]
        changes.append({"provider": instance, "pending": pending, "restored": case.status()})
    before = pages(case)
    first_context = int(await case.pages[0].locator('[data-plugin="service-dependencies"]').get_attribute("data-context"))
    first = next(item for item in before if item["context"] == first_context)
    remaining = next(item for item in before if item["context"] != first_context)
    await case.pages[0].close()
    await case.wait(lambda: all(item["group"] != first["group"] for item in case.status()["instances"]), "关闭页面的实例回收")
    await process(case.pages[1], "ＡＢ🌍", "AB🌍 · 3 字符")
    after = pages(case)
    assert len(after) == 1 and after[0]["context"] == remaining["context"]
    assert after[0]["revision"] == remaining["revision"]
    result = {"providers": ["Rust", "Python", "TypeScript"], "initial_pages": initial,
              "changes": changes, "remaining": after, "value": "AB🌍 · 3 字符"}
    case.save("dependencies.json", result)
    await case.pages[1].screenshot(path=str(case.project / "dependencies-desktop.png"), full_page=True)
    await case.pages[1].set_viewport_size({"width": 390, "height": 844})
    await case.pages[1].screenshot(path=str(case.project / "dependencies-mobile.png"), full_page=True)
    return {"providers": result["providers"], "pending_and_restored": len(changes), "remaining_pages": 1, "value": result["value"]}
