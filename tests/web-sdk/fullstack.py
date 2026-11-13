import ast
import json
from pathlib import Path
import shutil
import tomllib


def provider(case, directory, version):
    manifest = directory / "cordis-plugin.toml"
    declared = tomllib.loads(manifest.read_text())
    source = directory / "kv_py/__main__.py"
    module = ast.parse(source.read_text())
    entry = next(statement.value for statement in reversed(module.body)
                 if isinstance(statement, ast.Expr) and isinstance(statement.value, ast.Call)
                 and isinstance(statement.value.func, ast.Name) and statement.value.func.id == "run")
    entry.keywords.append(ast.keyword(arg="version", value=ast.Constant(version)))
    source.write_text(ast.unparse(ast.fix_missing_locations(module)) + "\n")
    command = ["conda", "run", "--no-capture-output", "-n", "base", "python", "-B", "-m", "kv_py"]
    lines = [f'name={json.dumps(declared["name"])}', f'version={json.dumps(version)}',
             'socket="process"', 'protocol=1', f'command={json.dumps(command)}', '[web]']
    lines += [f'{key}={json.dumps(value)}' for key, value in declared["web"].items()]
    manifest.write_text("\n".join(lines) + "\n")


def version(case):
    group = next(group for group in case.status()["groups"] if group["name"] == "kv")
    return group["plugins"][0]["version"]


async def restored(case, origins):
    for index, page in enumerate(case.pages):
        panel = page.locator('[data-plugin="kv-dashboard-python"]')
        await panel.get_by_role("heading", name="Python 数据便笺").wait_for()
        assert await panel.get_by_label("本地计数", exact=True).inner_text() == str(index + 1)
        await panel.get_by_label("存储键", exact=True).fill("cross-language")
        await panel.get_by_label("存储内容", exact=True).fill("待读取")
        await panel.get_by_role("button", name="读取内容").click()
        field = panel.get_by_label("存储内容", exact=True)
        await page.wait_for_function("field => field.value === '跨语言共享内容'", arg=await field.element_handle())
        assert await page.evaluate("performance.timeOrigin") == origins[index]


async def run(case):
    origins = [await page.evaluate("performance.timeOrigin") for page in case.pages]
    for index, page in enumerate(case.pages):
        panel = page.locator('[data-plugin="kv-dashboard-rust"]')
        await panel.get_by_role("heading", name="Rust 数据便笺").wait_for()
        for _ in range(index + 1):
            await panel.get_by_role("button", name="计数加一").click()
    panel = case.pages[0].locator('[data-plugin="kv-dashboard-rust"]')
    await panel.get_by_label("存储键", exact=True).fill("cross-language")
    await panel.get_by_label("存储内容", exact=True).fill("跨语言共享内容")
    await panel.get_by_role("button", name="写入存储").click()
    await panel.get_by_role("status").filter(has_text="内容已写入存储").wait_for()
    await case.pages[0].screenshot(path=str(case.project / "fullstack-rust.png"), full_page=True)
    candidate = case.project / "sources/python"
    shutil.copytree(case.examples / "plugins/kv-dashboard-python", candidate)
    provider(case, candidate, "0.2.0")
    await case.replace("store", candidate)
    await case.wait(lambda: version(case) == "0.2.0", "Python后端版本生效")
    await restored(case, origins)
    await case.pages[0].screenshot(path=str(case.project / "fullstack-python.png"), full_page=True)
    failed = case.candidate(case.examples / "plugins/kv-dashboard-python", "python-rejected", "rejected.js")
    provider(case, failed, "0.3.0")
    rejected = await case.replace("store", failed, accepted=False)
    assert "候选页面激活失败" in str(rejected), rejected
    await case.wait(lambda: version(case) == "0.2.0", "失败候选的后端版本恢复")
    await restored(case, origins)
    await case.pages[0].screenshot(path=str(case.project / "fullstack-restored.png"), full_page=True)
    await case.pages[1].set_viewport_size({"width": 390, "height": 844})
    await case.pages[1].screenshot(path=str(case.project / "fullstack-mobile.png"), full_page=True)
    return {"native_to_python": True, "backend_restored": "0.2.0", "page_counts": [1, 2], "shared_value": "跨语言共享内容", "rejected": rejected}
