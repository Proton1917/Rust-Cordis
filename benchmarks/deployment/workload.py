import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import time

from benchmarks.multilanguage.measure import Running
from .memory import sample


ROOT = Path(__file__).resolve().parents[2]
PAIRS = 10
SIZES = {"state-1MiB": 1 << 20, "state-4MiB": 4 << 20}
SCENARIOS = (*SIZES, "rpc-64B")


@dataclass(frozen=True)
class Inputs:
    binary: Path
    probe: Path
    work: Path


def run(inputs, scenario, case, seed=False):
    project = inputs.work / case
    plugins = project / "plugins"
    plugins.mkdir(parents=True)
    (project / "tmp").mkdir()
    store = inputs.binary / f"libkv_mem{inputs.probe.suffix}"
    for library in (store, inputs.probe):
        shutil.copyfile(library, plugins / library.name)
    if scenario in SIZES:
        length = SIZES[scenario]
        component = "state-probe"
        group = "store"
        config = f'bytes={length}, action="{("seed" if seed else "restore")}"'
        report = "inherited.json"
        checkpoint = inputs.work / (scenario + ".state")
        if not seed:
            (project / "state").mkdir()
            shutil.copyfile(checkpoint, project / "state/store.state")
    else:
        component, group, report = "latency", "consumer", "rpc-latency.json"
        config = (f'samples=10000, warmup=1000, key="read-key", label="rpc-64B", '
                  f'value={list(range(64))}')
    (project / "cordis.toml").write_text(
        '[runtime]\ncheckpoint_ms=0\nconfirm_ms=0\nrestart_max=0\n'
        f'[instance.store]\nplugin="{store.name}"\ncomponent="kv-store"\ngroup="store"\n'
        f'[instance.probe]\nplugin="{inputs.probe.name}"\ncomponent="{component}"\ngroup="{group}"\n'
        f'config={{ {config} }}\n'
    )
    started = time.perf_counter_ns()
    live = Running(project, inputs.binary, ROOT, None)
    published = project / "state/data/cordis-latency-probe" / report
    try:
        while not published.is_file():
            if live.process.poll() is not None or time.perf_counter_ns() - started > 30_000_000_000:
                raise RuntimeError(f"{case} 探针未交付结果，现场位于 {project}")
            time.sleep(0.002)
        elapsed = time.perf_counter_ns() - started
        measured = json.loads(published.read_bytes())
        # 状态页按周期发布。计时止于业务值核对，随后取得对应的正式宿主 PID。
        expected_groups = 1 if scenario in SIZES else 2
        def active_groups():
            snapshot = live.status()
            if not snapshot or snapshot.get("phase") != "running":
                return None
            groups = snapshot["groups"]
            if len(groups) == expected_groups and all(g["pid"] for g in groups):
                return groups
            return None
        groups = live.wait(active_groups, "读取当前宿主进程", seconds=15)
        data = {"core": sample(live.process.pid),
                "hosts": {group["name"]: sample(group["pid"]) for group in groups}}
        if scenario in SIZES:
            if measured != length:
                raise ValueError(f"{case} 恢复值长度为 {measured}")
            data.update(restored_bytes=measured, start_to_verified_ns=elapsed)
        else:
            if measured["samples"] != 10_000 or len(measured["sample_ns"]) != 10_000:
                raise ValueError(f"{case} 调用样本缺项")
            data.update(measured, calls_per_second=10_000 * 1e9 / measured["wall_ns"])
    finally:
        live.stop()
    if seed:
        shutil.copyfile(project / "state/store.state", checkpoint)
    shutil.rmtree(project)
    return data


def binary_identity(path):
    stat = path.stat()
    return {"path": str(path), "bytes": stat.st_size, "mtime_ns": stat.st_mtime_ns}


def main():
    cli = argparse.ArgumentParser()
    cli.add_argument("baseline", type=Path)
    cli.add_argument("candidate", type=Path)
    cli.add_argument("probe", type=Path)
    cli.add_argument("output", type=Path)
    cli.add_argument("--scenario", choices=SCENARIOS, action="append",
                     help="本次改动涉及的场景；默认执行全部场景")
    args = cli.parse_args()
    scenarios = tuple(dict.fromkeys(args.scenario or SCENARIOS))
    work = args.output.resolve()
    work.mkdir(parents=True, exist_ok=False)
    probe = args.probe.resolve()
    builds = {label: Inputs(binary.resolve(), probe, work) for label, binary in (
        ("baseline", args.baseline), ("candidate", args.candidate)
    )}
    metadata = {"platform": platform.platform(), "cpu_count": os.cpu_count(), "pairs": PAIRS,
                "scenarios": scenarios, "probe": binary_identity(probe),
                "bins": {key: [binary_identity(inputs.binary / name) for name in
                               ("cordis", "cordis-host", f"libkv_mem{probe.suffix}")]
                         for key, inputs in builds.items()}}
    (work / "inputs.json").write_text(json.dumps(metadata, indent=2))
    for scenario in scenarios:
        if scenario in SIZES:
            run(builds["baseline"], scenario, "seed-" + scenario, seed=True)
    with (work / "workload.jsonl").open("x") as out:
        for pair in range(PAIRS + 1):
            for scenario in scenarios:
                for label in (builds if pair % 2 == 0 else reversed(builds)):
                    data = run(builds[label], scenario, f"{pair}-{label}-{scenario}")
                    print(json.dumps(dict(pair=pair, scenario=scenario, build=label, **data)),
                          file=out, flush=True)


if __name__ == "__main__":
    def stop(signum, frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, stop)
    main()
