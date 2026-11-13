import json
from pathlib import Path
import shutil
import signal
import subprocess
import time

from support import EVIDENCE, manifest, test_environment


def wait_for(process, operation, description):
    deadline = time.monotonic() + 20
    while True:
        value = operation()
        if value is not None:
            return value
        if process.poll() is not None or time.monotonic() >= deadline:
            raise RuntimeError(description)
        time.sleep(0.02)


def execute():
    project = EVIDENCE / "migration"
    project.mkdir()
    plugins = project / "plugins"
    plugins.mkdir()
    fixture = EVIDENCE / "bin/cordis-runtime-fixture"
    for library in ("libkv_mem.dylib", "libcounter.dylib"):
        shutil.copy2(EVIDENCE / "bin" / library, plugins / library)
    for role in ("admin", "kv-client"):
        manifest(plugins / role, f"fixture-{role}", [str(fixture), role, "v1"])
    python = ["conda", "run", "--no-capture-output", "-n", "base", "python"]
    manifest(project / "sources/python", "kv", python + ["-m", "kv_py"])
    manifest(
        project / "sources/failed",
        "kv",
        python + [str(Path(__file__).with_name("failed_provider.py"))],
    )
    config = '[runtime]\ncheckpoint_ms=100\nconfirm_ms=30\n[instance.store]\nplugin="kv"\ncomponent="kv-store"\n[instance.counter]\nplugin="counter"\ncomponent="counter"\nconfig={interval_ms=10}\n'
    for name, role in (("admin", "admin"), ("client", "kv-client")):
        config += f'[instance.{name}]\nplugin="fixture-{role}"\ncomponent="{role}"\n'
        if name == "admin":
            config += 'grants={"cordis.manage@1"=["*"]}\n'
        config += (
            "config={commands="
            + json.dumps(str(project / (name + "-commands")))
            + ",results="
            + json.dumps(str(project / (name + "-results")))
            + "}\n"
        )
    (project / "cordis.toml").write_text(config)
    environment = test_environment()
    with (
        (project / "stdout.log").open("w") as output,
        (project / "stderr.log").open("w") as error,
    ):
        process = subprocess.Popen(
            [str(EVIDENCE / "bin/cordis"), "run"],
            cwd=project,
            env=environment,
            stdout=output,
            stderr=error,
        )
        try:
            wait_for(
                process,
                lambda: True if (project / "client-commands").is_dir() else None,
                "客户端启动超时",
            )
            serial = 0

            def command(target, method, args):
                nonlocal serial
                serial += 1
                path = project / (target + "-commands") / f"{serial:03}.json"
                staged = path.with_suffix(".next")
                interface = "kv@1.0" if target == "client" else "cordis.manage@1.1"
                staged.write_text(
                    json.dumps({"interface": interface, "method": method, "args": args})
                )
                staged.rename(path)
                response = project / (target + "-results") / path.name
                return wait_for(
                    process,
                    lambda: (
                        json.loads(response.read_text()) if response.exists() else None
                    ),
                    f"{method}回复超时",
                )

            def counter():
                response = command("client", "get", ["count"])
                assert response["ok"], response
                return int.from_bytes(bytes(response["value"]), "little")

            before = counter()
            installed = command("admin", "install", [str(project / "sources/python")])
            assert installed["ok"], installed
            replaced = command("admin", "replace", ["store", installed["value"]])
            assert replaced["ok"], replaced
            time.sleep(0.3)
            python_count = counter()
            assert python_count > before, (before, python_count)
            failed = command("admin", "install", [str(project / "sources/failed")])
            assert failed["ok"], failed
            rejected = command("admin", "replace", ["store", failed["value"]])
            assert not rejected["ok"] and rejected["code"] == 1001, rejected
            time.sleep(0.3)
            after_failure = counter()
            assert after_failure > python_count, (python_count, after_failure)
            back = command("admin", "replace", ["store", "libkv_mem.dylib"])
            assert back["ok"], back
            time.sleep(0.2)
            native_count = counter()
            assert native_count > after_failure, (after_failure, native_count)
            restored = command("admin", "replace", ["store", installed["value"]])
            assert restored["ok"], restored
            ticket = command(
                "admin",
                "replace_batch",
                [[["store", "libkv_mem.dylib"]], False, "python-native"],
            )
            assert ticket["ok"], ticket

            def batch_outcome():
                snapshot = json.loads((project / "state/status.json").read_text())
                return next(
                    (
                        entry
                        for entry in snapshot["last_batch"]
                        if entry["caller"] == "admin"
                        and entry["id"] == "python-native"
                        and entry["state"] in ("succeeded", "rejected")
                    ),
                    None,
                )

            outcome = wait_for(process, batch_outcome, "批次替换结果超时")
            assert outcome["state"] == "succeeded", outcome
            batch_count = counter()
            assert batch_count >= native_count, (native_count, batch_count)
            result = {
                "native_before": before,
                "python": python_count,
                "python_after_rejected_candidate": after_failure,
                "native_after": native_count,
                "batch_native_after": batch_count,
                "batch": outcome,
                "rejection": rejected,
                "periodic_checkpoint_ms": 100,
            }
            process.send_signal(signal.SIGTERM)
            assert process.wait(timeout=15) == 0
            (project / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


execute()
