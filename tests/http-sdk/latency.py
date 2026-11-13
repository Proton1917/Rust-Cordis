import os
import platform
import statistics
import time


def run(case):
    """RM-11：每种语言固定 10000 次 GET，计入发送至响应正文读取完成。"""
    measurements = {}
    for language, prefix in case.prefixes.items():
        samples = []
        connection = case.connection()
        try:
            for _ in range(10000):
                started = time.perf_counter_ns()
                connection.request("GET", prefix + "/version")
                response = connection.getresponse()
                body = response.read()
                samples.append(time.perf_counter_ns() - started)
                assert response.status == 200 and body
        finally:
            connection.close()
        ordered = sorted(samples)
        measurements[language] = {
            "count": len(samples),
            "median_ns": statistics.median(samples),
            "p99_ns": ordered[9899],
            "samples_ns": samples,
        }
    return {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "logical_cpus": os.cpu_count(),
        "transport": "HTTP/1.1 keep-alive，loopback TCP，三种处理方各自独立分组",
        "timing": "发送 GET 前至完整响应正文读取后；包含 HTTP、宿主、核心转发与处理方往返",
        "warmup": 0,
        "measurements": measurements,
    }
