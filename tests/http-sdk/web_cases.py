import asyncio
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

from playwright.async_api import async_playwright

from measure import runtime, save
from run import Case


SECRET = "m22-page-backend-private-value"
COOKIES = ["m22-page-one-cookie", "m22-page-two-cookie"]


def prepare(case):
    case.prepare()
    root, project = case.options.workspace, case.project
    plugins = project / "plugins"
    identity = plugins / "identity"
    (identity / "web").mkdir(parents=True)
    shutil.copy2(case.options.tools / "libhttp_test_native.dylib", identity / "libhttp_test_native.dylib")
    shutil.copy2(case.options.tools / "libhttp_test_native.dylib", plugins / "libhttp_test_native.dylib")
    shutil.copy2(root / "tests/http-sdk/fixtures/web/index.js", identity / "web/index.js")
    for name in ("identity.js", "secrets.js"):
        shutil.copy2(root / "tests/http-sdk/process/dist" / name, identity / "web" / name)
    (identity / "cordis-plugin.toml").write_text('name="http-tests"\nversion="0.1.0"\nsocket="native"\nlibrary="libhttp_test_native.dylib"\n[web]\nassets="web"\nmount="/identity"\nentry="index.js"\n')
    shutil.copy2(case.options.tools / "libcordis_secrets.dylib", plugins / "libcordis_secrets.dylib")
    secret = project / "private.env"
    secret.write_text("ALPHA=" + SECRET + "\n")
    secret.chmod(0o600)
    inspector = plugins / "inspector"
    inspector.mkdir()
    command = [sys.executable, "-B", str(root / "tests/http-sdk/inspector.py")]
    (inspector / "cordis-plugin.toml").write_text(f'name="page-inspector"\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n')
    configuration = '[instance.secrets]\nplugin="libcordis_secrets.dylib"\ncomponent="secrets"\nconfig={file="private.env",grants={identity=["ALPHA"]}}\n'
    configuration += '[instance.identity]\nplugin="identity"\ncomponent="identity-provider"\ngroup="identity"\n'
    configuration += f'[instance.backend-probe]\nplugin="libhttp_test_native.dylib"\ncomponent="identity-probe"\ngroup="backend-probe"\nconfig={{result={json.dumps(str(project / "backend.json"))}}}\n'
    configuration += f'[instance.inspector]\nplugin="inspector"\ncomponent="inspector"\nconfig={{commands={json.dumps(str(project / "inspect-commands"))},results={json.dumps(str(project / "inspect-results"))}}}\n'
    with (project / "cordis.toml").open("a") as output:
        output.write(configuration)
    case.required.update({"secrets", "identity", "backend-probe", "inspector"})


async def wait(condition, description, seconds=35):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        result = await condition()
        if result:
            return result
        await asyncio.sleep(0.03)
    raise TimeoutError(description)


async def attributes(case, key):
    number = len(list((case.project / "inspect-results").glob("*.json"))) + 1
    name = f"{number:04}.json"
    pending = case.project / "inspect-commands" / (name + ".next")
    pending.write_text(json.dumps({"key": key}))
    pending.rename(pending.with_name(name))
    result = case.project / "inspect-results" / name
    async def ready():
        return result.exists()
    await wait(ready, "HTTP 页面属性响应")
    return json.loads(result.read_text())


async def report(page):
    await page.locator("#identity-report").wait_for()
    return json.loads(await page.locator("#identity-report").get_attribute("data-report"))


async def conflicts(case):
    result = case.project / "conflicts.json"
    addition = f'[instance.conflicts]\nplugin="libhttp_test_native.dylib"\ncomponent="route-conflicts"\ngroup="conflicts"\nconfig={{prefixes=["/api","/_cordis/entry","/identity","/identity/child","/"],result={json.dumps(str(result))}}}\n'
    with (case.project / "cordis.toml").open("a") as output:
        output.write(addition)
    await asyncio.to_thread(case.manage, "reload", [])
    await asyncio.to_thread(case.current.wait, result.exists, "冲突登记结果")
    assert (await asyncio.to_thread(case.request, "GET", "/api/nested/value"))[2] == b"nested-route"
    assert (await asyncio.to_thread(case.request, "GET", "/api/version"))[0] == 200
    await asyncio.to_thread(case.manage, "remove", ["conflicts"])
    candidate = case.project / "sources/conflicting-web"
    (candidate / "web").mkdir(parents=True)
    (candidate / "web/index.js").write_text("export default {name:'conflict',async apply(){}};\n")
    (candidate / "cordis-plugin.toml").write_text('name="conflicting-web"\nversion="0.1.0"\nsocket="web"\n[web]\nassets="web"\nmount="/api/sub"\nentry="index.js"\n')
    installed = await asyncio.to_thread(case.manage, "install", [str(candidate)])
    reply, _ = await asyncio.to_thread(case.current.request, "admin", "cordis.manage@1.1", "add", ["conflicting-web", installed["value"], None, {}], expect_success=False)
    assert reply["ok"] is False
    assert (await asyncio.to_thread(case.request, "GET", "/api/version"))[0] == 200
    return {"routes": json.loads(result.read_text()), "web_after_route": reply, "longest_prefix": True}


async def pages(case, context, url):
    observed, opened = [], []
    websocket_urls = []
    for cookie in COOKIES:
        await context.add_cookies([{"url": url, "name": "session", "value": cookie, "httpOnly": True}])
        page = await context.new_page()
        page.on("websocket", lambda socket: websocket_urls.append(socket.url))
        await page.goto(url)
        current = await report(page)
        assert current["metadata"]["cordis.caller"] == {"page_of": "identity"}, current
        assert dict(current["attributes"]["headers"])["cookie"] == "session=" + cookie
        assert current["secret_obtained"] and current["secretCode"] == 9 and current["forgedCode"] == 9
        assert await page.evaluate("document.cookie") == ""
        assert "cordis" not in (await page.title() + await page.locator("body").inner_text()).lower()
        observed.append(current)
        opened.append(page)
    keys = [item["metadata"]["cordis.page"]["key"] for item in observed]
    assert keys[0] != keys[1] and all(len(key) >= 16 for key in keys)
    assert all(item["metadata"]["cordis.page"]["forwarder"] == "http" for item in observed)
    assert dict((await attributes(case, keys[0]))["headers"])["cookie"] == "session=" + COOKIES[0]
    backend = json.loads((case.project / "backend.json").read_text())
    assert backend["metadata"]["cordis.caller"] == {"instance": "backend-probe"}
    assert "cordis.page" not in backend["metadata"]
    secret = json.loads((case.project / "state/data/http-tests/backend-secret.json").read_text())
    assert secret == {"value": SECRET, "requested_by_page": True}
    await opened[0].screenshot(path=str(case.project / "default-shell.png"), full_page=True)
    await opened[0].close()
    async def released():
        return await attributes(case, keys[0]) is None
    await wait(released, "关闭页面的属性撤回")
    assert await attributes(case, keys[1]) is not None
    assert all(address.startswith(url.replace("http:", "ws:") + "_cordis/socket") for address in websocket_urls)
    return opened[1], {"pages": observed, "backend": backend, "closed_key_released": True, "websockets": websocket_urls}


async def restart(case, page):
    before = await report(page)
    group = next(group for group in case.current.status()["groups"] if group["name"] == "http")
    core_pid = case.current.process.pid
    os.kill(group["pid"], signal.SIGKILL)
    async def restored():
        item = await page.locator("#identity-report").get_attribute("data-report") if await page.locator("#identity-report").count() else None
        current = json.loads(item) if item else None
        return current if current and current["metadata"]["cordis.page"]["key"] != before["metadata"]["cordis.page"]["key"] else None
    observed = await wait(restored, "HTTP 恢复后的页面重连与服务重新登记")
    for prefix in case.prefixes.values():
        assert (await asyncio.to_thread(case.request, "GET", prefix + "/version"))[0] == 200
    assert case.current.process.pid == core_pid and case.current.process.poll() is None
    return {"core_pid": core_pid, "old_http_pid": group["pid"], "restored_page": observed}


async def shell(case, context, url, http_config):
    directory = case.project / "shell"
    directory.mkdir()
    source = '<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>应用外壳</title><style>body{margin:24px;font:16px/1.6 system-ui,sans-serif}main{margin-top:24px}</style><script src="/_cordis/session.js"></script><body><header id="app-shell">应用外壳</header><main id="cordis-components"></main><script type="module">import {connect} from "/_cordis/cordis-web.js"; globalThis.shellMark=crypto.randomUUID(); connect({token:CORDIS_SESSION.token});</script></body></html>'
    (directory / "index.html").write_text(source)
    await asyncio.to_thread(case.manage, "configure", ["http", {**http_config, "assets": "shell", "index": "index.html", "title": "内部配置标题"}])
    page = await context.new_page()
    await page.goto(url)
    await report(page)
    before = await page.evaluate("({time:performance.timeOrigin,mark:shellMark,title:document.title})")
    assert before["title"] == "应用外壳"
    candidate = case.project / "sources/identity-next"
    shutil.copytree(case.project / "plugins/identity", candidate)
    entry = candidate / "web/index.js"
    entry.write_text(entry.read_text().replace("调用来源", "更新后的调用来源"))
    installed = await asyncio.to_thread(case.manage, "install", [str(candidate)])
    await asyncio.to_thread(case.manage, "replace", ["identity", installed["value"]])
    await page.get_by_role("heading", name="更新后的调用来源").wait_for()
    after = await page.evaluate("({time:performance.timeOrigin,mark:shellMark,title:document.title})")
    assert after == before and await page.locator("#app-shell").inner_text() == "应用外壳"
    await page.screenshot(path=str(case.project / "custom-shell-replaced.png"), full_page=True)
    await page.set_viewport_size({"width": 390, "height": 844})
    await page.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
    assert await page.locator("#app-shell").count() == 1 and await page.locator("#identity-report").count() == 1
    assert await page.evaluate("document.documentElement.scrollWidth <= innerWidth")
    await page.screenshot(path=str(case.project / "custom-shell-mobile.png"), full_page=True)
    missing = await context.new_page()
    response = await missing.goto(url + "missing-page")
    assert response.status == 404
    assert "cordis" not in (await missing.title() + await missing.locator("body").inner_text()).lower()
    await missing.screenshot(path=str(case.project / "not-found.png"), full_page=True)
    return {"before": before, "after": after, "not_found": 404}


async def execute(options):
    case = Case(options)
    prepare(case)
    proxy = None
    with runtime(case.project, options) as current:
        case.start(current)
        with (case.project / "proxy.log").open("w") as log:
            proxy = subprocess.Popen(["node", str(options.workspace / "tests/http-sdk/process/dist/proxy.js"), "http://" + case.address, str(case.project / "proxy.json")], stdout=log, stderr=log)
        try:
            current.wait(lambda: (case.project / "proxy.json").exists(), "反向代理监听")
            info = json.loads((case.project / "proxy.json").read_text())
            origin = f'http://127.0.0.1:{info["port"]}'
            config = {"max_body_bytes": 104857600, "public_origins": [origin]}
            case.manage("configure", ["http", config])
            current.wait(case.ready, "HTTP 公开来源生效")
            async with async_playwright() as playwright:
                environment = {**os.environ, "TMPDIR": str(case.project / "tmp")}
                context = await playwright.chromium.launch_persistent_context(str(case.project / "browser-profile"), channel="chrome", headless=True, viewport={"width": 1100, "height": 850}, env=environment)
                errors = []
                def attach(page):
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.on("console", lambda message: errors.append(message.text) if message.type == "error" else None)
                context.on("page", attach)
                try:
                    page, identity = await pages(case, context, origin + "/")
                    token = json.loads(case.request("GET", "/_cordis/session.json")[2])["token"]
                    status, _, _ = case.request("GET", "/_cordis/socket", headers={"Connection":"Upgrade", "Upgrade":"websocket", "Sec-WebSocket-Version":"13", "Sec-WebSocket-Key":"dGhlIHNhbXBsZSBub25jZQ==", "Origin":"https://forbidden.example", "Sec-WebSocket-Protocol":"cordis-up1,cordis-token." + token})
                    assert status == 403
                    collision = await conflicts(case)
                    restored = await restart(case, page)
                    customized = await shell(case, context, origin + "/", config)
                    save(case.project / "observed-status.json", current.status())
                finally:
                    save(case.project / "browser-errors.json", errors)
                    save(case.project / "before-cleanup-status.json", current.status())
                    await context.close()
        finally:
            if proxy is not None:
                proxy.terminate()
                assert proxy.wait(timeout=10) == 0
    paths = [case.project / "stdout.log", case.project / "stderr.log", case.project / "state/status.json", *list((case.project / "state").rglob("*.state"))]
    for path in paths:
        if path.exists():
            assert all(value.encode() not in path.read_bytes() for value in [SECRET, *COOKIES]), path
    return {"identity": identity, "conflicts": collision, "restart": restored, "shell": customized, "proxy": info, "invalid_origin": 403, "private_core_outputs": [str(path.relative_to(case.project)) for path in paths if path.exists()]}


def run(options):
    return asyncio.run(execute(options))
