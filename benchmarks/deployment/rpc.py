"""Paired, cross-process KV reads using byte-identical caller/provider plugins."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import statistics
import sys
import time

import psutil

from benchmarks.multilanguage.measure import Running
from .memory import sample
from .workload import binary_identity


ROOT = Path(__file__).resolve().parents[2]
SIZES = (64, 1 << 20, 4 << 20)


def save(path, value):
    staged = path.with_suffix(path.suffix + ".next")
    staged.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    staged.replace(path)


def identity(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return dict(binary_identity(path), sha256=digest.hexdigest())


def session_members(session):
    """Host subprocesses have separate process groups within this owned session."""
    members = []
    for process in psutil.process_iter():
        try:
            if os.getsid(process.pid) == session:
                members.append(process)
        except (ProcessLookupError, PermissionError, psutil.NoSuchProcess):
            pass
    return members


def stop(live):
    problem = None
    try:
        live.stop()
    except Exception as error:
        problem = error
    # A killed core can leave hosts in their own process groups. The independently
    # created session limits cleanup to this run, including reparented children.
    members = session_members(live.process.pid)
    for process in members:
        try:
            process.terminate()
        except psutil.NoSuchProcess:
            pass
    _, remaining = psutil.wait_procs(members, timeout=5)
    for process in remaining:
        try:
            process.kill()
        except psutil.NoSuchProcess:
            pass
    _, remaining = psutil.wait_procs(remaining, timeout=5)
    alive = []
    for process in remaining:
        try:
            if process.is_running() and process.status() != psutil.STATUS_ZOMBIE:
                alive.append(process.pid)
        except psutil.NoSuchProcess:
            pass
    save(live.project / "cleanup.json", {
        "core_exit": live.process.returncode,
        "fallback_processes": [process.pid for process in members],
        "remaining_processes": alive,
        "graceful_error": str(problem) if problem else None,
    })
    if alive:
        raise RuntimeError(f"measurement processes remain alive: {alive}")
    if problem:
        raise problem


def validate(report, size, samples, warmup, concurrency=1):
    values = report["sample_ns"]
    if (report["samples"] != samples or len(values) != samples
            or report["payload_bytes"] != size or report["warmup"] != warmup
            or report.get("concurrency", 1) != concurrency
            or not all(isinstance(value, int) and value >= 0 for value in values)
            or report["wall_ns"] <= 0):
        raise ValueError("probe report does not match the configured workload")
    ordered = sorted(values)
    if (report["median_ns"] != statistics.median(ordered)
            or report["p99_ns"] != ordered[(samples * 99 + 99) // 100 - 1]):
        raise ValueError("probe quantiles do not match its raw samples")


def run(options, label, tools, size, pair, phase, order):
    load = "" if options.concurrency == 1 else f"-c{options.concurrency}"
    case = f"{phase}-{pair:02d}-{size}{load}-{label}"
    project = options.output / case
    plugins = project / "plugins"
    plugins.mkdir(parents=True)
    (project / "tmp").mkdir()
    for source in (options.provider, options.probe):
        destination = plugins / source.name
        shutil.copy2(source, destination)
        if identity(destination)["sha256"] != options.plugin_hashes[str(source)]:
            raise ValueError(f"plugin input changed: {source}")
    samples = options.samples if size == 64 else options.large_samples
    warmup = options.warmup if size == 64 else options.large_warmup
    configuration = (
        '[runtime]\ncheckpoint_ms=0\nconfirm_ms=0\nrestart_max=0\n'
        f'lifecycle_timeout_ms={int(options.timeout * 1000) + 5000}\n'
        f'[instance.store]\nplugin={json.dumps(options.provider.name)}\n'
        'component="kv-store"\ngroup="store"\n'
        f'[instance.probe]\nplugin={json.dumps(options.probe.name)}\n'
        'component="latency"\ngroup="consumer"\n'
        f'config={{samples={samples},warmup={warmup},bytes={size},concurrency={options.concurrency},'
        f'key="read-key",label={json.dumps(case)}}}\n'
    )
    (project / "cordis.toml").write_text(configuration)
    started = time.perf_counter_ns()
    started_utc = datetime.now(timezone.utc).isoformat()
    live = Running(project, tools, ROOT, None, start_new_session=True)
    published = project / "state/data/cordis-latency-probe/rpc-latency.json"
    failed = False
    try:
        def completed():
            snapshot = live.status()
            if snapshot and (snapshot.get("instance_errors") or any(
                    item["state"].lower() == "failed" for item in snapshot["instances"])):
                raise RuntimeError(f"runtime instance failed: {snapshot}")
            if (published.is_file() and snapshot and snapshot.get("phase") == "running"
                    and len(snapshot["instances"]) == 2
                    and all(item["state"].lower() == "active" for item in snapshot["instances"])):
                return snapshot
            return None

        snapshot = live.wait(completed, f"{case}: probe completion", seconds=options.timeout)
        startup_to_report_ns = time.perf_counter_ns() - started
        report = json.loads(published.read_bytes())
        validate(report, size, samples, warmup, options.concurrency)
        groups = {group["name"]: group for group in snapshot["groups"]}
        if (set(groups) != {"consumer", "store"}
                or len({live.process.pid, groups["consumer"]["pid"], groups["store"]["pid"]}) != 3):
            raise ValueError("the probe and provider must occupy two separate native hosts")
        memory = {"core": dict(pid=live.process.pid, **sample(live.process.pid))}
        memory.update({name: dict(pid=group["pid"], **sample(group["pid"]))
                       for name, group in groups.items()})
        result = dict(pair=pair, phase=phase, order_in_pair=order, build=label,
                      payload_bytes=size, concurrency=options.concurrency, started_utc=started_utc,
                      startup_to_report_ns=startup_to_report_ns,
                      calls_per_second=samples * 1e9 / report["wall_ns"],
                      measurement=report, after_report_process_memory_bytes=memory,
                      runtime_status=snapshot)
        save(project / "measurement.json", result)
    except BaseException:
        failed = True
        raise
    finally:
        try:
            stop(live)
        except Exception:
            if not failed:
                raise
            print(f"cleanup failed; inspect {project / 'cleanup.json'}", file=sys.stderr)
    result["cleanup"] = json.loads((project / "cleanup.json").read_text())
    save(options.output / f"{case}.json", result)
    if not options.keep_runs:
        shutil.rmtree(project)
    return result


def positive(value):
    value = int(value)
    if value <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return value


def nonnegative(value):
    value = int(value)
    if value < 0:
        raise argparse.ArgumentTypeError("must be nonnegative")
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("baseline", "candidate", "probe", "provider", "output"):
        parser.add_argument(name, type=Path)
    parser.add_argument("--pairs", "--rounds", type=positive, default=5)
    parser.add_argument("--warmup-pairs", type=nonnegative, default=1)
    parser.add_argument("--samples", type=positive, default=2000)
    parser.add_argument("--warmup", type=nonnegative, default=200)
    parser.add_argument("--large-samples", type=positive, default=128)
    parser.add_argument("--large-warmup", type=nonnegative, default=8)
    parser.add_argument("--concurrency", type=positive, default=1)
    parser.add_argument("--size", action="append", type=int, choices=SIZES)
    parser.add_argument("--timeout", type=positive, default=120)
    parser.add_argument("--keep-runs", action="store_true")
    options = parser.parse_args()
    if os.name != "posix":
        parser.error("the paired runtime measurement requires POSIX process sessions")
    for name in ("baseline", "candidate", "probe", "provider", "output"):
        setattr(options, name, getattr(options, name).resolve())
    if options.probe.name == options.provider.name:
        parser.error("probe and provider filenames must differ")
    sizes = list(dict.fromkeys(options.size or SIZES))
    for size in sizes:
        samples = options.samples if size == 64 else options.large_samples
        if options.concurrency > samples:
            parser.error("concurrency must be at most the total sample count for every selected size")
    builds = {"baseline": options.baseline, "candidate": options.candidate}
    binaries = {label: {name: identity(tools / name) for name in ("cordis", "cordis-host")}
                for label, tools in builds.items()}
    plugins = {name: identity(getattr(options, name)) for name in ("probe", "provider")}
    options.plugin_hashes = {entry["path"]: entry["sha256"] for entry in plugins.values()}
    options.output.mkdir(parents=True, exist_ok=False)
    save(options.output / "inputs.json", {
        "platform": platform.platform(), "machine": platform.machine(), "cpu_count": os.cpu_count(),
        "cpu_affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
        "pairs": options.pairs, "warmup_pairs": options.warmup_pairs, "sizes": sizes,
        "samples_64B": options.samples, "warmup_64B": options.warmup,
        "samples_large": options.large_samples, "warmup_large": options.large_warmup,
        "concurrency": options.concurrency,
        "binaries": binaries, "shared_plugins": plugins,
        "timing": ("serial Service<Kv>::get through caller host, core and provider host; returned Vec freed before each sample ends"
                   if options.concurrency == 1 else
                   "concurrent synchronous Service<Kv>::get through caller host, core and provider host; each sample includes full-byte result verification and Vec destruction"),
        "wall_ns": ("probe read loop, including per-call timer and sample-vector push; excludes setup and warmup"
                    if options.concurrency == 1 else
                    "shared worker release through last worker completion, including per-call timer, verification and sample-vector push; excludes thread creation, serial warmup, joins and sample merging"),
        "memory": "per-process bytes sampled after report; Linux peak_rss is VmHWM since process start, including startup, setup and report publication",
        "pss": "proportional resident memory at sampling; rss counts shared pages in every process",
        "comparison": "process memory observations are separate from DTO requested-heap allocation counters",
    })
    with (options.output / "rpc.jsonl").open("x") as output:
        count = options.warmup_pairs + options.pairs
        for index in range(count):
            phase = "warmup_pair" if index < options.warmup_pairs else "formal"
            pair = index if phase == "warmup_pair" else index - options.warmup_pairs + 1
            labels = list(builds) if index % 2 == 0 else list(reversed(builds))
            for size in sizes:
                for order, label in enumerate(labels, start=1):
                    result = run(options, label, builds[label], size, pair, phase, order)
                    print(json.dumps(result, ensure_ascii=False), file=output, flush=True)


if __name__ == "__main__":
    def interrupted(signum, frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    main()
