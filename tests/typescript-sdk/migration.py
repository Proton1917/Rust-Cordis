import itertools
import json
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

from acceptance import EVIDENCE, PLUGIN, PROVIDER, environment, manifest, stop


def until(observe):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        value = observe()
        if value is not None:
            return value
        time.sleep(0.03)
    raise TimeoutError("状态迁移观察结果超过25秒")


def read(path):
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        return None


def fixture(path, role):
    path.mkdir(parents=True)
    command = [str(EVIDENCE / "bin/cordis-runtime-fixture"), role, "v1"]
    (path / "cordis-plugin.toml").write_text(
        f'name="fixture-{role}"\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n'
    )


def execute():
    project = EVIDENCE / (sys.argv[1] if len(sys.argv) > 1 else "migration-logical-kv")
    rollback = len(sys.argv) > 2 and sys.argv[2] == "rollback"
    project.mkdir()
    plugins = project / "plugins"
    fixture(plugins / "admin", "admin")
    fixture(plugins / "client", "kv-client")
    sources = project / "sources"
    manifest(sources / "typescript", "kv", PROVIDER)
    manifest(sources / "failed", "kv", PLUGIN / "failing.js")
    if rollback:
        manifest(plugins / "store", "kv", PROVIDER)
    for name in (["counter"] if rollback else ["kv_mem", "counter"]):
        shutil.copy2(EVIDENCE / f"bin/lib{name}.dylib", plugins / f"lib{name}.dylib")
    shutil.copy2(EVIDENCE / "bin/libkv_mem.dylib", sources / "libkv_mem.dylib")
    (project / "cordis.toml").write_text(
        '[runtime]\ncheckpoint_ms=0\nconfirm_ms=20\n'
        '[instance.store]\nplugin="kv"\ncomponent="kv-store"\n'
        '[instance.counter]\nplugin="counter"\ncomponent="counter"\nconfig={interval_ms=5}\n'
        '[instance.admin]\nplugin="admin"\ncomponent="admin"\ngrants={"cordis.manage@1"=["*"]}\n'
        f'config={{commands={json.dumps(str(project / "commands"))},results={json.dumps(str(project / "results"))}}}\n'
        '[instance.client]\nplugin="client"\ncomponent="kv-client"\n'
        f'config={{commands={json.dumps(str(project / "client-commands"))},results={json.dumps(str(project / "client-results"))}}}\n'
    )
    ids = itertools.count(1)

    def call(interface, method, args, success=True):
        prefix = "client-" if interface == "kv@1.0" else ""
        number = next(ids)
        path = project / f"{prefix}commands/{number}.json"
        path.parent.mkdir(exist_ok=True)
        temporary = path.with_suffix(".next")
        temporary.write_text(json.dumps({"interface": interface, "method": method, "args": args}))
        temporary.rename(path)
        result = until(lambda: read(project / f"{prefix}results/{number}.json"))
        assert result["ok"] == success, result
        return result["value"] if success else result

    def active():
        snapshot = read(project / "state/status.json")
        if snapshot and len(snapshot["instances"]) == 4 and all(item["state"] == "active" for item in snapshot["instances"]):
            return snapshot

    def count():
        return int.from_bytes(call("kv@1.0", "get", ["count"]), "little")

    def published(before, reference, rolled_back=False):
        snapshot = active()
        if snapshot is None:
            return None
        counter = next(item for item in snapshot["instances"] if item["name"] == "counter")
        store = next(item for item in snapshot["instances"] if item["name"] == "store")
        restored = any(report["group"] == "kv" and report["rolled_back"] for report in snapshot["last_replace"])
        if store["reference"] == reference and (restored if rolled_back else counter["context"] != before):
            return snapshot

    with (project / "stdout.log").open("w") as output, (project / "stderr.log").open("w") as error:
        process = subprocess.Popen([str(EVIDENCE / "bin/cordis"), "run"], cwd=project, env=environment(), stdout=output, stderr=error)
        try:
            initial = until(active)
            values = [count()]
            epochs = []
            selection = [sources / "failed"] if rollback else [sources / "typescript", sources / "libkv_mem.dylib"]
            for source in selection:
                previous = until(active)
                before = next(item["context"] for item in previous["instances"] if item["name"] == "counter")
                previous_reference = next(item["reference"] for item in previous["instances"] if item["name"] == "store")
                installed = call("cordis.manage@1.0", "install", [str(source)])
                replacement = call("cordis.manage@1.0", "replace", ["store", installed], success=not rollback)
                if rollback:
                    assert replacement["code"] == 1001, replacement
                snapshot = until(lambda: published(before, previous_reference if rollback else installed, rollback))
                after = next(item["context"] for item in snapshot["instances"] if item["name"] == "counter")
                epochs.append([before, after])
                values.append(count())
                assert values[-1] >= values[-2], values
                if rollback:
                    advanced = until(lambda: value if (value := count()) > values[-1] else None)
                    values.append(advanced)
            stop(process)
            assert process.returncode == 0, process.returncode
            result = {"sequence": ["TypeScript", "failed candidate", "TypeScript"] if rollback else ["Rust", "TypeScript", "Rust"], "counts": values,
                      "counter_contexts": epochs, "core_pid": initial["pid"], "exit": 0}
            (project / "result.json").write_text(json.dumps(result, indent=2))
            print(json.dumps(result, indent=2))
        finally:
            stop(process)


if __name__ == "__main__":
    execute()
