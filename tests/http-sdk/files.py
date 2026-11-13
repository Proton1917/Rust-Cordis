import json
from pathlib import Path
import subprocess
import threading
import time


def equal(left, right):
    with Path(left).open("rb") as source, Path(right).open("rb") as received:
        while True:
            first, second = source.read(1024 * 1024), received.read(1024 * 1024)
            assert first == second, "文件字节不同"
            if not first:
                break


def rss(pid):
    return int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True).strip()) * 1024


def transfer(case, language, prefix, source):
    size = source.stat().st_size
    with source.open("rb") as stream:
        status, _, body = case.request("PUT", prefix + "/file", stream, {"Content-Length": str(size)})
    assert status == 200 and json.loads(body)["stored"] == size, (status, body)
    plugin = {"rust": "http-echo", "python": "http-echo-py", "typescript": "http-echo-ts"}[language]
    stored = case.project / "state/data" / plugin / "upload.bin"
    equal(source, stored)
    target = case.project / (language + "-download.bin")
    connection = case.connection()
    try:
        connection.request("GET", prefix + "/file")
        response = connection.getresponse()
        assert response.status == 200 and int(response.getheader("Content-Length")) == size
        with target.open("wb") as output:
            while chunk := response.read(1024 * 1024):
                output.write(chunk)
    finally:
        connection.close()
    equal(source, target)
    return {"bytes": size, "uploaded_equal": True, "downloaded_equal": True}


def rejected_length(case, length):
    """ADR-0046 §3.4：已知长度超限在读取正文前返回 413。"""
    connection = case.connection()
    try:
        connection.putrequest("POST", "/api/file")
        connection.putheader("Content-Length", str(length))
        connection.endheaders()
        response = connection.getresponse()
        response.read()
        assert response.status == 413, response.status
    finally:
        connection.close()


def run(case):
    source = case.project / "source.bin"
    chunk = bytes(range(256)) * 4096
    with source.open("wb") as output:
        for _ in range(64):
            output.write(chunk)
    pid = next(group["pid"] for group in case.current.status()["groups"] if group["name"] == "http")
    baseline = rss(pid)
    samples = [baseline]
    stop = threading.Event()
    def measure():
        while not stop.wait(0.02):
            samples.append(rss(pid))
    monitor = threading.Thread(target=measure)
    monitor.start()
    try:
        transfers = {language: transfer(case, language, prefix, source) for language, prefix in case.prefixes.items()}
    finally:
        stop.set()
        monitor.join()
    increase = max(samples) - baseline
    assert increase < 32 * 1024 * 1024, {"baseline": baseline, "maximum": max(samples), "increase": increase}
    case.manage("configure", ["rust", {"prefix": "/api", "max_body_bytes": 1048576}])
    rejected_length(case, 1048576 + 1)
    status, _, _ = case.request("POST", "/api/file", iter([chunk, b"x"]), chunked=True)
    assert status == 413, status
    return {"transfers": transfers, "http_pid": pid, "rss_baseline": baseline, "rss_maximum": max(samples), "rss_increase": increase, "rss_samples": samples, "oversized_status": 413}
