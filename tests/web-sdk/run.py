import argparse
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
import tomllib

from playwright.async_api import async_playwright


def scenario_module(options, name):
    directory = Path(__file__).parent if name in ("fullstack", "dependencies") else options.modules
    spec = importlib.util.spec_from_file_location(name, directory / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Case:
    def __init__(self, options):
        self.options = options
        self.project = options.evidence.resolve()
        self.tools = options.tools.resolve()
        self.examples = options.examples.resolve()
        self.process = None
        self.context = None
        self.pages = []
        self.errors = []
        self.console_errors = []
        self.serial = 0
        self.url = ""

    def save(self, name, value):
        (self.project / name).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")

    def status(self):
        return json.loads((self.project / "state/status.json").read_text())

    async def wait(self, condition, description, timeout=35):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.process.returncode is not None:
                raise RuntimeError(f"核心退出 {self.process.returncode}：{description}")
            value = condition()
            if value:
                return value
            await asyncio.sleep(0.05)
        raise TimeoutError(description)

    async def manage(self, method, arguments):
        self.serial += 1
        filename = f"{self.serial:03}.json"
        path = self.project / "commands" / filename
        staged = path.with_suffix(".next")
        staged.write_text(json.dumps({"interface": "cordis.manage@1.1", "method": method, "args": arguments}))
        staged.rename(path)
        result = self.project / "results" / filename
        return await self.wait(lambda: json.loads(result.read_text()) if result.exists() else None, f"管理调用{method}完成")

    async def session(self):
        response = await self.context.request.get(self.url + "_cordis/session.json")
        assert response.ok
        return await response.json()

    def manifest(self, directory, name, command):
        directory.mkdir(parents=True)
        (directory / "cordis-plugin.toml").write_text(
            f'name={json.dumps(name)}\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n'
        )

    def fixture(self, name, plugins):
        source = self.options.modules / "fixtures" / name
        destination = plugins / name
        shutil.copytree(source, destination)
        with (self.project / f"{name}-build.log").open("w") as log:
            subprocess.run([
                "node", str(Path(__file__).parent / "build-fixture.mjs"),
                str(self.options.modules.resolve()), str(source.resolve()), str(destination),
            ], check=True, stdout=log, stderr=subprocess.STDOUT)
        return tomllib.loads((source / "cordis-plugin.toml").read_text())

    def prepare(self):
        self.project.mkdir(parents=True)
        plugins = self.project / "plugins"
        plugins.mkdir()
        shutil.copy2(self.tools / "libcordis_http.dylib", plugins / "libcordis_http.dylib")
        shutil.copytree(self.examples / "plugins/web-notes", plugins / "notes")
        self.manifest(plugins / "admin", "fixture-admin", [str(self.tools / "cordis-runtime-fixture"), "admin", "v1"])
        configuration = '[runtime]\ncheckpoint_ms=0\nconfirm_ms=20\n'
        if self.options.scenario == "channel-faults":
            configuration += 'invoke_timeout_ms=200\n'
        configuration += '[instance.http]\nplugin="http"\nforwarder=true\n[instance.notes]\nplugin="web-notes"\n'
        configuration += '[instance.admin]\nplugin="fixture-admin"\ncomponent="admin"\ngrants={"cordis.manage@1"=["*"]}\nconfig={commands=' + json.dumps(str(self.project / "commands")) + ',results=' + json.dumps(str(self.project / "results")) + '}\n'
        if self.options.scenario in ("fullstack", "batch"):
            shutil.copytree(self.examples / "plugins/kv-dashboard-rust", plugins / "store")
            shutil.copy2(self.tools / "libkv_mem.dylib", plugins / "store/libkv_mem.dylib")
            configuration += '[instance.store]\nplugin="kv"\ncomponent="kv-store"\n'
        if self.options.scenario == "channel-faults":
            for name in ("flood-provider", "flood-client"):
                metadata = self.fixture(name, plugins)
                configuration += f'[instance.{name}]\nplugin={json.dumps(metadata["name"])}\n'
        if self.options.scenario == "realm-close":
            metadata = self.fixture("realm-counter", plugins)
            configuration += f'[instance.realm-counter]\nplugin={json.dumps(metadata["name"])}\n'
        if self.options.scenario == "dependencies":
            configuration += scenario_module(self.options, "dependencies").prepare(self, plugins)
        (self.project / "cordis.toml").write_text(configuration)
        (self.project / "tmp").mkdir()

    async def start(self):
        environment = os.environ.copy()
        environment["PATH"] = str(self.tools) + os.pathsep + environment["PATH"]
        environment["TMPDIR"] = str(self.project / "tmp")
        if self.options.dependencies:
            environment["PYTHONPATH"] = str(self.options.dependencies.resolve())
        with (self.project / "stdout.log").open("w") as output, (self.project / "stderr.log").open("w") as error:
            self.process = await asyncio.create_subprocess_exec(str(self.tools / "cordis"), "run", cwd=self.project, env=environment, stdout=output, stderr=error)
        address = self.project / "state/data/http/listeners/http.json"
        await self.wait(lambda: address.exists(), "默认HTTP入口地址发布")
        self.url = "http://" + json.loads(address.read_text())["bound"] + "/"
        await self.wait(lambda: (self.project / "commands").is_dir(), "管理实例启动")
        return environment

    async def open_pages(self):
        for _ in range(2):
            page = await self.context.new_page()
            page.on("pageerror", lambda problem: self.errors.append(str(problem)))
            page.on("console", lambda message: self.console_errors.append({"text": message.text, "location": message.location}) if message.type == "error" else None)
            self.pages.append(page)
            await page.goto(self.url)
            await page.get_by_label("笔记内容").wait_for()
        await self.wait(lambda: len([item for item in self.status()["instances"] if item["name"].endswith("-notes") and item["state"] == "active"]) == 2, "两页笔记子实例激活")
        await self.wait(lambda: all(item["state"] == "active" for item in self.status()["instances"]), "全部已配置组件激活")

    def candidate(self, source, label, wrapper):
        destination = self.project / "sources" / label
        shutil.copytree(source, destination)
        shutil.copy2(destination / "web/index.js", destination / "web/base.js")
        shutil.copy2(Path(__file__).parent / "fixtures" / wrapper, destination / "web/index.js")
        return destination

    async def replace(self, instance, source, accepted=True):
        installed = await self.manage("install", [str(source)])
        assert installed["ok"], installed
        result = await self.manage("replace", [instance, installed["value"]])
        assert result["ok"] is accepted, result
        return result


async def notes_replacement(case):
    first, second = case.pages
    await first.get_by_label("笔记内容").fill("第一页的笔记")
    await second.get_by_label("笔记内容").fill("第二页的笔记")
    await first.get_by_role("button", name="计数加一").click()
    await second.get_by_role("button", name="计数加一").click()
    await second.get_by_role("button", name="计数加一").click()
    origins = [await page.evaluate("performance.timeOrigin") for page in case.pages]
    candidate = case.candidate(case.examples / "plugins/web-notes", "notes-next", "notes-next.js")
    await case.replace("notes", candidate)
    for page, text, count in [(first, "第一页的笔记", "1"), (second, "第二页的笔记", "2")]:
        await page.get_by_role("heading", name="工作笔记").wait_for()
        assert await page.get_by_label("笔记内容").input_value() == text
        assert await page.get_by_label("本地计数", exact=True).inner_text() == count
    rejected = case.candidate(case.examples / "plugins/web-notes", "notes-rejected", "rejected.js")
    await case.replace("notes", rejected, accepted=False)
    for index, page in enumerate(case.pages):
        await page.get_by_role("heading", name="工作笔记").wait_for()
        assert await page.evaluate("performance.timeOrigin") == origins[index]
    assert await first.get_by_label("笔记内容").input_value() == "第一页的笔记"
    assert await second.get_by_label("笔记内容").input_value() == "第二页的笔记"
    await first.screenshot(path=str(case.project / "restored-desktop.png"), full_page=True)
    await second.set_viewport_size({"width": 390, "height": 844})
    await second.screenshot(path=str(case.project / "restored-mobile.png"), full_page=True)
    return {"state_inherited": True, "rollback": True, "page_reload": False}


async def restart_http(case):
    before = case.status()
    original = next(group for group in before["groups"] if group["name"] == "http")
    session = await case.session()
    os.kill(original["pid"], signal.SIGKILL)
    await case.wait(lambda: any(group["name"] == "http" and group["state"] == "running" and group["pid"] != original["pid"] for group in case.status()["groups"]), "HTTP进程按默认地址恢复")
    await case.wait(lambda: len([item for item in case.status()["instances"] if item["name"].endswith("-notes") and item["state"] == "active"]) == 2, "两页重连并重建")
    current = await case.session()
    assert current["url"].split("#")[0] == case.url
    assert current["token"] != session["token"]
    for page in case.pages:
        await page.get_by_role("heading", name="页面笔记").wait_for()
        assert await page.locator('[data-plugin="web-notes"]').count() == 1
        await page.get_by_role("button", name="计数加一").click()
        assert await page.get_by_label("本地计数", exact=True).inner_text() == "1"
    await case.pages[0].screenshot(path=str(case.project / "http-restored.png"), full_page=True)
    return {"same_origin": case.url, "token_rotated": True, "pages_active": 2}


async def execute(options):
    case = Case(options)
    case.prepare()
    result = None
    try:
        environment = await case.start()
        async with async_playwright() as playwright:
            case.context = await playwright.chromium.launch_persistent_context(str(case.project / "browser-profile"), channel="chrome", headless=True, viewport={"width": 1100, "height": 850}, env=environment)
            try:
                await case.open_pages()
                if options.scenario == "pure-replace":
                    result = await notes_replacement(case)
                elif options.scenario == "http-restart":
                    result = await restart_http(case)
                elif options.scenario in ("channel-faults", "realm-close", "scaffold", "batch", "fullstack", "dependencies"):
                    name = options.scenario.replace("-", "_")
                    module = scenario_module(options, name)
                    result = await module.run(case)
                else:
                    raise ValueError("该场景入口正在接入")
                assert not case.errors, case.errors
            finally:
                case.save("observed-status.json", case.status())
                case.save("page-errors.json", case.errors)
                case.save("console-errors.json", case.console_errors)
                for index, page in enumerate(case.pages):
                    if not page.is_closed():
                        await page.screenshot(path=str(case.project / f"observed-{index}.png"), full_page=True)
                await case.context.close()
    finally:
        if (case.project / "state/status.json").exists():
            case.save("final-status.json", case.status())
        if case.process is not None:
            if case.process.returncode is None:
                case.process.send_signal(signal.SIGTERM)
            try:
                await asyncio.wait_for(case.process.wait(), 40)
            except TimeoutError:
                case.process.kill()
                await case.process.wait()
                raise
    assert case.process.returncode == 0, case.process.returncode
    case.save("result.json", {"scenario": options.scenario, "exit": case.process.returncode, "result": result})
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--tools", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--examples", type=Path, required=True)
    parser.add_argument("--dependencies", type=Path)
    parser.add_argument("--modules", type=Path, default=Path(__file__).parent)
    parser.add_argument("--scenario", choices=("pure-replace", "fullstack", "realm-close", "http-restart", "channel-faults", "scaffold", "batch", "dependencies"), required=True)
    arguments = parser.parse_args()
    if arguments.dependencies:
        sys.path.insert(0, str(arguments.dependencies.resolve()))
    asyncio.run(execute(arguments))
