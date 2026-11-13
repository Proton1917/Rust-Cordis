import asyncio
import base64
import time

import cbor2


CAPTURE_SOCKET = """() => {
    const NativeSocket = globalThis.WebSocket;
    globalThis.cordisFaultSockets = [];
    globalThis.WebSocket = class extends NativeSocket {
        constructor(...arguments_) {
            super(...arguments_);
            const record = {socket: this, closed: false, code: null, received: []};
            globalThis.cordisFaultSockets.push(record);
            this.addEventListener('close', event => {
                record.closed = true;
                record.code = event.code;
            });
            this.addEventListener('message', event => {
                if (record.capture && event.data instanceof ArrayBuffer) {
                    record.received.push(Array.from(new Uint8Array(event.data)));
                }
            });
        }
    };
}"""

SEND_FAULT = """({kind, frames}) => {
    const record = globalThis.cordisFaultSockets.find(item => item.socket.readyState === WebSocket.OPEN);
    if (!record) throw new Error('故障页面的实际连接缺席');
    record.capture = true;
    const socket = record.socket;
    let sent = 0;
    if (kind === 'oversized') {
        socket.send(new Uint8Array(16 * 1024 * 1024 + 1));
        sent = 1;
    } else {
        for (const encoded of frames) {
            socket.send(Uint8Array.from(atob(encoded), value => value.charCodeAt(0)));
            sent++;
        }
    }
    globalThis.cordisFaultRecord = record;
    return sent;
}"""


def encoded(value):
    return base64.b64encode(cbor2.dumps(value)).decode("ascii")


def identity(snapshot):
    """AU-15：保留健康页面和 HTTP 的实际实例世代。"""
    return {
        item["name"]: (item["context"], item["revision"], item["state"])
        for item in snapshot["instances"]
    }


async def observe_fault(page, kind, sent):
    """CP-3/4：固定窗口等待故障页结束，并保存真实调用峰值和容量拒绝。"""
    deadline = time.monotonic() + 35
    errors = []
    while time.monotonic() < deadline:
        result = await page.evaluate("""() => {
            const record = globalThis.cordisFaultRecord;
            return {closed: record.closed, code: record.code, frames: record.received.splice(0)};
        }""")
        for frame in result.pop("frames"):
            response = cbor2.loads(bytes(frame)).get("response")
            if response and response["id"] >= 2**48:
                errors.append(response["result"]["error"]["code"])
        if result["closed"]:
            result["sent"] = sent
            if kind == "flood":
                metrics = await page.evaluate("globalThis.cordisFloodMetrics")
                assert sent == 128
                assert metrics["peak"] == 64, metrics
                assert 8 in errors, errors
                result["calls"] = metrics
                result["error_codes"] = sorted(set(errors))
            return result
        await asyncio.sleep(0.05)
    raise TimeoutError(f"{kind} 在35秒观察窗口内连接保持打开")


async def healthy_pages(case, original):
    """AU-15：故障后由实际控件确认两张健康页面继续处理操作。"""
    assert case.process.returncode is None
    current = case.status()
    assert current["pid"] == original["pid"]
    current_instances = identity(current)
    for name, expected in identity(original).items():
        assert current_instances[name] == expected, (name, expected, current_instances[name])
    original_http = next(group for group in original["groups"] if group["name"] == "http")
    current_http = next(group for group in current["groups"] if group["name"] == "http")
    assert current_http["connection"] == original_http["connection"]
    assert current_http["pid"] == original_http["pid"]
    counts = []
    for page in case.pages:
        output = page.get_by_label("本地计数", exact=True).first
        before = int(await output.inner_text())
        await page.get_by_role("button", name="计数加一", exact=True).first.click()
        await page.wait_for_function(
            "({root, value}) => Number(root.textContent) === value",
            arg={"root": await output.element_handle(), "value": before + 1},
        )
        counts.append(int(await output.inner_text()))
    return counts


async def one_fault(case, kind, frames):
    """DOC-16 §11：向已有真实页面连接注入单次故障，等待该页实例回收。"""
    original = case.status()
    previous_groups = {group["name"] for group in original["groups"]}
    page = await case.context.new_page()
    fault_groups = set()
    result = None
    try:
        await page.add_init_script(f"({CAPTURE_SOCKET})()")
        await page.goto(case.url)
        await page.get_by_role("button", name="计数加一", exact=True).first.wait_for()

        def attached():
            snapshot = case.status()
            groups = {group["name"] for group in snapshot["groups"]} - previous_groups
            children = [item for item in snapshot["instances"] if item["group"] in groups]
            return groups if children and all(item["state"] == "active" for item in children) else None

        fault_groups = await case.wait(attached, f"{kind} 页面子实例进入 Active")
        if kind == "flood":
            alias = int(await page.evaluate("globalThis.cordisFloodObject"))
            frames = [
                encoded({"request": {"id": 2**48 + index, "method": "invoke", "params": {
                    "object": alias, "method": "pause", "args": [], "refs": [],
                }}})
                for index in range(128)
            ]
        sent = await page.evaluate(SEND_FAULT, {"kind": kind, "frames": frames})
        result = await observe_fault(page, kind, sent)
        assert result["closed"], result
    finally:
        await page.close()
    await case.wait(
        lambda: all(item["group"] not in fault_groups for item in case.status()["instances"]),
        f"{kind} 页面子实例回收",
    )
    result["healthy_counts"] = await healthy_pages(case, original)
    result["closed_groups"] = sorted(fault_groups)
    result["remaining_instances"] = identity(case.status())
    case.save(f"channel-{kind}.json", result)
    return result


async def run(case):
    """AU-15、CP-2/3/8：四个固定故障窗口覆盖消息预算和页面隔离。"""
    await case.wait(
        lambda: all(item["state"] == "active" for item in case.status()["instances"]),
        "健康页面及等待服务进入 Active",
    )
    nested = encoded({"notification": {"method": "channel_open", "params": {"channel": 1, "metadata": None}}})
    results = {}
    for kind, frames in (
        ("oversized", []),
        ("malformed", [base64.b64encode(b"\xff").decode("ascii")]),
        ("flood", []),
        ("nested", [nested]),
    ):
        results[kind] = await one_fault(case, kind, frames)
    case.save("channel-faults.json", results)
    return results
