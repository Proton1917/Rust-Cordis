import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import time


VALUE = list(range(64))
KEY = "cordis-measurement"


def save(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def copy_plugin(source, destination):
    """DOC-15 §9：运行目录取得完整插件交付物，源码位置作为输入记录保存。"""
    source = source.resolve()
    if source.is_dir():
        shutil.copytree(source, destination, ignore=shutil.ignore_patterns("__pycache__", ".git", "target"))
    else:
        destination = destination.with_suffix(source.suffix)
        shutil.copy2(source, destination)
    return destination.name


def fixture(project, tools, role):
    directory = project / "plugins" / role
    directory.mkdir()
    command = [str(tools / "cordis-runtime-fixture"), role, "v1"]
    (directory / "cordis-plugin.toml").write_text(
        f'name="fixture-{role}"\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n'
    )
    grant = 'grants={"cordis.manage@1"=["*"]}\n' if role == "admin" else ""
    return f'[instance.{role}]\nplugin="{role}"\ncomponent="{role}"\ngroup="{role}"\n{grant}config={{commands={json.dumps(str(project / (role + "-commands")))},results={json.dumps(str(project / (role + "-results")))}}}\n'


class Running:
    def __init__(self, project, tools, workspace, dependencies, python_paths=(), arguments=()):
        self.project = project
        self.serial = 0
        environment = os.environ.copy()
        environment["PATH"] = str(tools) + os.pathsep + environment["PATH"]
        imports = [str(workspace / "sdks/python")]
        if dependencies:
            imports.append(str(dependencies.resolve()))
        imports.extend(str(path.resolve()) for path in python_paths)
        environment["PYTHONPATH"] = os.pathsep.join(imports)
        environment["TMPDIR"] = str(project / "tmp")
        with (project / "stdout.log").open("w") as output, (project / "stderr.log").open("w") as error:
            self.process = subprocess.Popen([str(tools / "cordis"), "run", *arguments], cwd=project, env=environment, stdout=output, stderr=error)

    def wait(self, observe, description, seconds=180):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"核心提前退出{self.process.returncode}：{description}")
            result = observe()
            if result:
                return result
            time.sleep(0.02)
        raise TimeoutError(description)

    def status(self):
        path = self.project / "state/status.json"
        return json.loads(path.read_text()) if path.exists() else None

    def active(self, name):
        snapshot = self.status()
        if snapshot:
            return next((item for item in snapshot["instances"] if item["name"] == name and item["state"] == "active"), None)

    def request(self, role, interface, method, arguments):
        self.serial += 1
        filename = f"{self.serial:03}.json"
        command = self.project / f"{role}-commands" / filename
        staged = command.with_suffix(".next")
        save(staged, {"interface": interface, "method": method, "args": arguments})
        started = time.perf_counter_ns()
        staged.rename(command)
        result = self.project / f"{role}-results" / filename
        reply = self.wait(lambda: json.loads(result.read_text()) if result.exists() else None, f"{method}完成")
        elapsed = time.perf_counter_ns() - started
        assert reply["ok"], reply
        return reply, elapsed

    def stop(self):
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
        try:
            code = self.process.wait(timeout=40)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            raise
        save(self.project / "exit.json", {"exit": code})
        assert code == 0, code


@contextmanager
def runtime(project, options, python_paths=(), arguments=()):
    running = Running(project, options.tools, options.workspace, options.dependencies, python_paths, arguments)
    try:
        yield running
    finally:
        running.stop()


def project(options, label, provider):
    directory = options.evidence / label
    (directory / "plugins").mkdir(parents=True)
    (directory / "tmp").mkdir()
    reference = copy_plugin(provider, directory / "plugins/store")
    configuration = '[runtime]\ncheckpoint_ms=0\nconfirm_ms=20\napply_timeout_ms=120000\nrestart_max=0\n'
    configuration += f'[instance.store]\nplugin={json.dumps(reference)}\ncomponent="kv-store"\ngroup="store"\nconfig={{path="measurement.cbor"}}\n'
    return directory, configuration


def rpc(options):
    """RM-7：每个方向固定10000次真实读取，完整时延由同一个Rust使用方计量。"""
    providers = {
        "rust-to-rust": options.tools / "libkv_mem.dylib",
        "rust-to-python": options.python_provider or options.workspace / "plugins/kv-py",
        "rust-to-typescript": options.typescript_provider or options.workspace / "plugins/kv-ts",
    }
    results = {}
    for label, provider in providers.items():
        directory, configuration = project(options, label, provider)
        name = copy_plugin(options.probe, directory / "plugins/probe")
        configuration += f'[instance.probe]\nplugin={json.dumps(name)}\ncomponent="latency"\ngroup="probe"\nconfig={{samples=10000,key={json.dumps(KEY)},value={VALUE},label={json.dumps(label)}}}\n'
        (directory / "cordis.toml").write_text(configuration)
        with runtime(directory, options) as running:
            path = directory / "state/data/cordis-latency-probe/rpc-latency.json"
            running.wait(lambda: path.exists() and running.active("probe"), "固定读取计量完成")
            report = json.loads(path.read_text())
            assert report["samples"] == 10000 and len(report["sample_ns"]) == 10000
            snapshot = running.status()
            groups = {group["name"]: group for group in snapshot["groups"]}
            assert groups["probe"]["pid"] != groups["store"]["pid"]
            assert groups["probe"]["plugins"][0]["format"] == 1
            report["runtime"] = snapshot
            report["caller"] = {"kind": "native-rust-plugin", "group": "probe", "pid": groups["probe"]["pid"], "build_info": groups["probe"]["plugins"][0]["build_info"]}
            report["timing"] = "Service<Kv>::get入口前至返回Vec释放后，包含SDK/ABI、Host-Core-提供方IPC往返、处理、解码与复制"
            report["provider_source"] = str(provider.resolve())
            save(directory / "measurement.json", report)
            results[label] = report
    return results


def replacement(options):
    """RM-8、ADR-0035：一次真实替换分别记录pause、管理RPC与外部观察耗时。"""
    directory, configuration = project(options, options.label, options.provider)
    configuration += fixture(directory, options.tools, "admin")
    configuration += fixture(directory, options.tools, "kv-client")
    (directory / "sources").mkdir()
    copy_plugin(options.candidate, directory / "sources/candidate")
    candidate = next((directory / "sources").iterdir())
    (directory / "cordis.toml").write_text(configuration)
    with runtime(directory, options) as running:
        before = running.wait(lambda: running.active("store") and running.active("kv-client") and running.active("admin") and running.status(), "测量实例激活")
        running.request("kv-client", "kv@1.0", "put", [KEY, VALUE])
        installed, _ = running.request("admin", "cordis.manage@1.1", "install", [str(candidate)])
        old = next(item for item in before["instances"] if item["name"] == "store")
        socket = next(item["entry"]["socket"] for item in before["sources"] if item["reference"] == old["reference"])
        assert installed["value"] != old["reference"], "候选需产生新的安装引用以执行实际替换"
        reply, wall_ns = running.request("admin", "cordis.manage@1.1", "replace", ["store", installed["value"]])
        after = running.wait(lambda: replacement_ready(running.status(), old, socket), "替换后提供方、依赖方和报告完成")
        restored, _ = running.request("kv-client", "kv@1.0", "get", [KEY])
        assert restored["value"] == VALUE, restored
        report = replacement_report(after, socket)
        assert report["succeeded"] and report["committed"], report
        result = {"label": options.label, "socket": socket, "provider": str(options.provider.resolve()), "candidate": str(options.candidate.resolve()), "samples": 1, "pause_ms": report["pause_ms"], "rpc_wall_us": reply["rpc_wall_us"], "harness_wall_ns": wall_ns, "state_bytes": VALUE, "replacement": report, "before": before, "after": after}
        save(directory / "measurement.json", result)
        return result


def replacement_report(snapshot, socket):
    field, key, value = ("last_swap", "plugin", "kv") if socket == "native" else ("last_replace", "group", "store")
    return next((item for item in snapshot[field] if item[key] == value), None)


def replacement_ready(snapshot, old, socket):
    if snapshot is None:
        return None
    instances = {item["name"]: item for item in snapshot["instances"]}
    report = replacement_report(snapshot, socket)
    if instances["store"]["state"] == "active" and instances["store"]["context"] != old["context"] and instances["kv-client"]["state"] == "active" and report and report["succeeded"]:
        return snapshot
    return None


def direct(options):
    """RM-9：复用M14驱动，固定9轮、每轮10000预热和100000完整调用。"""
    settings = {"baseline": str(options.baseline.resolve()), "current": str(options.current.resolve()), "rounds": 9, "warmup": 10000, "iterations": 100000, "median_ratio_limit": 1.05, "maximum_ratio_limit": 1.15}
    save(options.evidence / "settings.json", settings)
    command = [str(options.driver.resolve()), "--host", str(options.tools / "cordis-host"), "--provider", str(options.provider.resolve()), "--settings", str(options.evidence / "settings.json"), "--output", str(options.evidence / "direct")]
    with (options.evidence / "driver.log").open("w") as log:
        subprocess.run(command, cwd=options.workspace, stdout=log, stderr=subprocess.STDOUT, check=True)
    return json.loads((options.evidence / "direct/direct-call-measurements.json").read_text())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--tools", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--dependencies", type=Path)
    modes = parser.add_subparsers(dest="mode", required=True)
    calls = modes.add_parser("rpc")
    calls.add_argument("--probe", type=Path, required=True)
    calls.add_argument("--python-provider", type=Path)
    calls.add_argument("--typescript-provider", type=Path)
    replace = modes.add_parser("replace")
    replace.add_argument("--label", required=True)
    replace.add_argument("--provider", type=Path, required=True)
    replace.add_argument("--candidate", type=Path, required=True)
    typed = modes.add_parser("direct")
    for option in ("driver", "provider", "baseline", "current"):
        typed.add_argument("--" + option, type=Path, required=True)
    options = parser.parse_args()
    options.workspace = options.workspace.resolve()
    options.tools = options.tools.resolve()
    options.evidence = options.evidence.resolve()
    options.evidence.mkdir(parents=True)
    result = {"rpc": rpc, "replace": replacement, "direct": direct}[options.mode](options)
    save(options.evidence / "result.json", {"mode": options.mode, "machine": platform.machine(), "platform": platform.platform(), "logical_cpus": os.cpu_count(), "result": result})


if __name__ == "__main__":
    main()
