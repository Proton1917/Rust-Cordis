import argparse
import ctypes
import json
import pathlib
import platform
import shutil
import subprocess
import tempfile
import time

import psutil


def sample(pid):
    process = psutil.Process(pid)
    result = {"rss": process.memory_info().rss, "threads": process.num_threads()}
    if platform.system() == "Darwin":
        class Usage(ctypes.Structure):
            # Darwin sys/resource.h 中的 rusage_info_v4，由内核记录进程物理足迹峰值。
            _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [
                (field, ctypes.c_uint64) for field in (
                    "user_time", "system_time", "idle_wakes", "interrupt_wakes", "pageins",
                    "wired", "resident", "physical_footprint", "started", "exited",
                    "child_user", "child_system", "child_idle", "child_interrupt",
                    "child_pageins", "child_elapsed", "read_bytes", "written_bytes",
                    "qos_default", "qos_maintenance", "qos_background", "qos_utility",
                    "qos_legacy", "qos_initiated", "qos_interactive", "billed_system",
                    "serviced_system", "logical_writes", "peak_footprint", "instructions",
                    "cycles", "billed_energy", "serviced_energy", "interval_peak", "runnable",
                )
            ]
        usage = Usage()
        if ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True).proc_pid_rusage(
            pid, 4, ctypes.byref(usage)
        ):
            raise OSError(ctypes.get_errno(), "proc_pid_rusage")
        result["physical_footprint"] = usage.physical_footprint
        result["peak_physical_footprint"] = usage.peak_footprint
    else:
        result["pss"] = process.memory_full_info().pss
        with pathlib.Path(f"/proc/{pid}/status").open() as status:
            result["peak_rss"] = next(int(line.split()[1]) * 1024 for line in status if line.startswith("VmHWM:"))
    return result


def run(binary, scenario, root):
    suffix = "dylib" if platform.system() == "Darwin" else "so"
    members = {
        "empty": [], "one": ["kv_mem"], "four": ["kv_mem"] * 4,
        "shared": ["kv_mem", "counter", "console_log"],
        "separate": ["kv_mem", "counter", "console_log"],
    }[scenario]
    with tempfile.TemporaryDirectory(dir=root) as name:
        deploy = pathlib.Path(name)
        plugins = deploy / "plugins"
        plugins.mkdir()
        config = ["[runtime]", "checkpoint_ms = 0", "confirm_ms = 20"]
        for i, module in enumerate(members):
            library = f"lib{module}.{suffix}"
            if not (plugins / library).exists():
                shutil.copyfile(binary / library, plugins / library)
            group = f"worker{i}" if scenario == "separate" else "worker"
            config.extend([f"[instance.i{i}]", f'plugin = "{library}"', f'group = "{group}"'])
            if scenario == "four":
                config.append('isolate = { "kv@1" = true }')
        (deploy / "cordis.toml").write_text("\n".join(config))
        started = time.monotonic()
        core = subprocess.Popen([binary / "cordis", "run"], cwd=deploy,
                                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            state = deploy / "state/status.json"
            while time.monotonic() - started < 15:
                if core.poll() is not None:
                    raise RuntimeError(core.stderr.read().decode())
                if state.exists():
                    snapshot = json.loads(state.read_bytes())
                    failures = [i for i in snapshot["instances"] if i["state"].lower() == "failed"]
                    if snapshot.get("instance_errors") or failures:
                        raise RuntimeError((snapshot["instance_errors"], failures))
                    if (snapshot["phase"] == "running" and len(snapshot["instances"]) == len(members)
                        and all(i["state"].lower() == "active" for i in snapshot["instances"])):
                        break
                time.sleep(0.01)
            else:
                raise TimeoutError(f"{scenario}: {snapshot if state.exists() else '启动快照缺席'}")
            published = time.monotonic() - started
            time.sleep(0.2)
            return {"first_active_snapshot_s": published, "core": sample(core.pid),
                    "hosts": [sample(g["pid"]) for g in snapshot["groups"]]}
        finally:
            if core.poll() is None:
                core.terminate()
            _, stderr = core.communicate(timeout=40)
            if core.returncode:
                raise RuntimeError(stderr.decode())


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", type=pathlib.Path)
    parser.add_argument("candidate", type=pathlib.Path)
    parser.add_argument("directory", type=pathlib.Path)
    parser.add_argument("--pairs", type=int, default=10)
    parser.add_argument("--resume", action="store_true")
    args = parser.parse_args()
    paths = {"baseline": args.baseline.resolve(), "candidate": args.candidate.resolve()}
    args.directory.mkdir(parents=True, exist_ok=True)
    # 探索轮单列；正式样本按轮交替次序，文件独占创建以保留原始观测。
    file = args.directory / "memory.jsonl"
    previous = [json.loads(row) for row in file.read_text().splitlines()] if args.resume else []
    completed = {(row["pair"], row["scenario"], row["build"]) for row in previous}
    with file.open("a" if args.resume else "x") as output:
        for pair in range(args.pairs + 1):
            for scenario in ("empty", "one", "four", "shared", "separate"):
                for label in (paths if pair % 2 == 0 else reversed(paths)):
                    if (pair, scenario, label) in completed:
                        continue
                    data = run(paths[label], scenario, args.directory)
                    print(json.dumps(dict(pair=pair, scenario=scenario, build=label,
                                          system=platform.platform(), **data)),
                          file=output, flush=True)
