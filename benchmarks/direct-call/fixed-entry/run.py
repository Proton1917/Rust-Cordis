import argparse
import json
import math
import os
import pathlib
import platform
import shutil
import signal
import statistics
import subprocess
import tempfile
import time


SOURCE = pathlib.Path(__file__).resolve().parent
BASELINE = "2edc244cae71bd895d55b062fd20c3acd1318cb2"
ROUNDS, WARMUP, ITERATIONS, LATENCY_SAMPLES = 10, 10_000, 100_000, 2048


def budget(deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise TimeoutError("P8 测量与清理共用的截止时间已到")
    return remaining


def build(reference, work, deadline):
    # 两端作为独立 Cargo 工作区分别解析。初始依赖由同一基线锁文件固定。
    with tempfile.TemporaryDirectory(dir=reference / "benchmarks", prefix="p8-") as stage:
        directory = pathlib.Path(stage) / "fixed-entry"
        shutil.copytree(SOURCE, directory, ignore=shutil.ignore_patterns(
            "run.py", "README.md", "__pycache__", "target", "Cargo.lock"))
        for crate in ("provider", "consumer"):
            manifest = directory / crate / "Cargo.toml"
            shutil.copyfile(reference / "Cargo.lock", manifest.with_name("Cargo.lock"))
            command = ["cargo", "build", "--offline", "--release", "--manifest-path", manifest,
                       "--target-dir", work / "build"]
            with (work / f"{crate}-build.log").open("wb") as log:
                build_process = subprocess.Popen(command, cwd=reference, stdout=log,
                                                 stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    code = build_process.wait(timeout=budget(deadline))
                finally:
                    if build_process.poll() is None:
                        os.killpg(build_process.pid, signal.SIGKILL)
                        build_process.wait()
                if code:
                    raise subprocess.CalledProcessError(code, command)
            shutil.copyfile(manifest.with_name("Cargo.lock"), work / f"{crate}.lock")
    return work / "build/release"


def run(reference, work, deadline):
    binary = build(reference, work, deadline)
    extension = "dylib" if platform.system() == "Darwin" else "so"
    project = work / "app"
    plugins = project / "plugins"
    plugins.mkdir(parents=True)
    for name in ("provider", "consumer"):
        library = f"libfixed_entry_{name}.{extension}"
        shutil.copyfile(binary / library, plugins / library)
    provider = binary / f"libfixed_entry_provider.{extension}"
    raw = work / "raw.json"
    (project / "cordis.toml").write_text(
        f"""[runtime]
checkpoint_ms = 0
confirm_ms = 0
[instance.provider]
plugin = "fixed-entry-provider"
component = "provider"
group = "measurement"
config = {{ value = 100 }}
[instance.probe]
plugin = "fixed-entry-consumer"
component = "probe"
group = "measurement"
[instance.probe.config]
provider = {json.dumps(str(provider))}
output = {json.dumps(str(raw))}
initial = 100
rounds = {ROUNDS}
warmup = {WARMUP}
iterations = {ITERATIONS}
latency_samples = {LATENCY_SAMPLES}
"""
    )
    status = project / "state/status.json"
    with (work / "runtime.log").open("wb") as log:
        core = subprocess.Popen([reference / "target/release/cordis", "run"], cwd=project,
                                stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            while core.poll() is None:
                if status.is_file():
                    state = json.loads(status.read_bytes())
                    failures = [i for i in state["instances"] if i["state"] == "failed"]
                    if failures or state.get("instance_errors"):
                        raise RuntimeError((failures, state.get("instance_errors")))
                    if raw.is_file() and any(i["name"] == "probe" and i["state"] == "active"
                                             for i in state["instances"]):
                        return json.loads(raw.read_bytes())
                time.sleep(min(0.05, budget(deadline)))
            raise RuntimeError(f"核心提前退出，状态 {core.returncode}")
        finally:
            if core.poll() is None:
                core.terminate()
                try:
                    core.wait(timeout=min(5, max(0.01, deadline - time.monotonic())))
                except subprocess.TimeoutExpired:
                    os.killpg(core.pid, signal.SIGKILL)
                    core.wait()
            if core.returncode:
                raise RuntimeError(f"核心关闭状态 {core.returncode}，记录 {log.name}")


def report(samples):
    pairs = []
    for pair in range(1, ROUNDS + 1):
        rows = {row["path"]: row for row in samples if row["pair"] == pair}
        def measures(path):
            row = rows[path]
            latencies = sorted(row["latency_ns"])
            if len(latencies) != LATENCY_SAMPLES or row["calls"] != ITERATIONS:
                raise ValueError(f"配对 {pair} 的样本数与冻结计划不符")
            return row["elapsed_ns"] / row["calls"], latencies[math.ceil(.99 * len(latencies)) - 1]
        typed_ns, typed_p99 = measures("typed-v1")
        fixed_ns, fixed_p99 = measures("fixed-cbor")
        pairs.append(dict(pair=pair, typed_ns=typed_ns, fixed_ns=fixed_ns, typed_p99_ns=typed_p99,
                          fixed_p99_ns=fixed_p99, throughput_ratio=typed_ns / fixed_ns,
                          p99_ratio=fixed_p99 / typed_p99))
    throughput = [row["throughput_ratio"] for row in pairs]
    tail = [row["p99_ratio"] for row in pairs]
    if max(throughput) < .95 or min(tail) > 1.05:
        gate = "rejected"
    elif min(throughput) >= .95 and max(tail) <= 1.05:
        gate = "passed-scalar"
    else:
        gate = "inconclusive"
    return dict(reference=BASELINE, system=platform.platform(), rounds=ROUNDS, warmup=WARMUP,
                iterations=ITERATIONS, latency_samples=LATENCY_SAMPLES, pairs=pairs, scalar_gate=gate,
                median_throughput_ratio=statistics.median(throughput), median_p99_ratio=statistics.median(tail))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("reference", type=pathlib.Path)
    parser.add_argument("work", type=pathlib.Path)
    parser.add_argument("--seconds", type=float, required=True)
    args = parser.parse_args()
    deadline = time.monotonic() + args.seconds
    reference, work = args.reference.resolve(), args.work.resolve()
    work.mkdir(parents=True, exist_ok=False)
    compiler = subprocess.check_output(["rustc", "-Vv"], text=True, cwd=reference, timeout=budget(deadline))
    if not compiler.startswith("rustc 1.96.0 "):
        raise ValueError(f"预定编译器是 rustc 1.96.0，实际为 {compiler}")
    result = report(run(reference, work, deadline))
    result["compiler"] = compiler
    (work / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2))
    print(json.dumps({key: value for key, value in result.items() if key not in {"pairs", "compiler"}}))
