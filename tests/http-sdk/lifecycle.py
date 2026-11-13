from concurrent.futures import ThreadPoolExecutor
import json
import os
import shutil
import signal
import threading
import time

from measure import save


def version(case):
    status, _, body = case.request("GET", "/api/version")
    assert status == 200, (status, body)
    return json.loads(body)["version"]


class Traffic:
    def __init__(self, case):
        self.case = case
        self.pool = ThreadPoolExecutor(max_workers=16)
        self.stop = threading.Event()
        self.samples = []
        self.started = time.monotonic_ns()
        self.thread = threading.Thread(target=self.send)
        self.thread.start()

    def send(self):
        next_request = time.monotonic()
        while not self.stop.is_set():
            submitted = time.monotonic_ns()
            self.samples.append((submitted, self.pool.submit(version, self.case)))
            next_request += 0.02
            self.stop.wait(max(0, next_request - time.monotonic()))

    def finish(self):
        self.stop.set()
        self.thread.join()
        self.pool.shutdown(wait=True)
        return {"rate_per_second": 50, "start_ns": self.started,
            "requests": [{"submitted_ns": started, "status": 200, "version": future.result()} for started, future in self.samples]}


def replacement(case):
    sources = case.project / "sources"
    sources.mkdir()
    candidates = []
    for name in ("v1", "v2", "v3", "failed"):
        target = sources / (name + ".dylib")
        shutil.copy2(case.options.tools / ("libhttp_echo_" + name + ".dylib"), target)
        candidates.append(target)
    traffic = Traffic(case)
    replies = []
    try:
        case.current.wait(lambda: len(traffic.samples) >= 5, "持续请求进入稳定发送")
        for index, candidate in enumerate(candidates[:3], 1):
            installed = case.manage("install", [str(candidate)])
            reply = case.manage("replace", ["rust", installed["value"]])
            assert version(case) == f"0.1.{index}"
            replies.append(reply)
        installed = case.manage("install", [str(candidates[3])])
        rejected, _ = case.current.request("admin", "cordis.manage@1.1", "replace", ["rust", installed["value"]], expect_success=False)
        assert rejected["ok"] is False
        assert version(case) == "0.1.3"
        def rolled_back():
            return next((report for report in case.current.status().get("last_swap", []) if report["plugin"] == "http-echo" and report["rolled_back"]), None)
        rollback = case.current.wait(rolled_back, "回滚报告发布")
    finally:
        samples = traffic.finish()
        save(case.project / "replacement-traffic.json", samples)
    return {"accepted": replies, "rejected": rejected, "rollback": rollback, "traffic": samples}


def fault(case):
    case.manage("configure", ["python", {"prefix": "/python", "timeout_ms": 50}])
    status, headers, body = case.request("GET", "/python/wait")
    assert status == 504 and b"cordis" not in body.lower()
    assert case.request("GET", "/typescript/version")[0] == 200
    case.manage("configure", ["rust", {"prefix": "/api", "timeout_ms": 120000}])
    before = (case.project / "stderr.log").stat().st_size
    with ThreadPoolExecutor(max_workers=1) as pool:
        waiting = pool.submit(case.request, "GET", "/api/wait")
        def entered():
            with (case.project / "stderr.log").open("r") as log:
                log.seek(before)
                return "HTTP waiting request entered" in log.read()
        case.current.wait(entered, "真实处理方已进入等待", seconds=10)
        group = next(group for group in case.current.status()["groups"] if group["name"] == "rust")
        os.kill(group["pid"], signal.SIGKILL)
        assert case.request("GET", "/typescript/version")[0] == 200
        status, headers, body = waiting.result(timeout=10)
        assert status == 503 and b"cordis" not in body.lower(), (status, body)
    case.current.wait(lambda: case.current.active("rust") and case.request("GET", "/api/version")[0] == 200, "崩溃后的路由自动重新登记", seconds=15)
    return {"timeout": 504, "crashed_request": 503, "healthy_route": 200, "recovered_route": 200, "old_pid": group["pid"]}


def run(case):
    replaced = replacement(case)
    faults = fault(case)
    case.manage("remove", ["rust"])
    case.current.wait(lambda: case.request("GET", "/api/version")[0] == 404, "卸下后的宽限结束", seconds=10)
    return {"replacement": replaced, "faults": faults, "removed_status": 404}
