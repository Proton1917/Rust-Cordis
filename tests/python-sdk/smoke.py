import json
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time


from support import EVIDENCE, manifest, test_environment


def probe():
    from cordis_sdk import Component, run
    from kv_py import bindings

    async def apply(context, config):
        store = await context.get("kv@1.0", bindings, bindings.KvClient)
        assert await store.get("missing") is None
        await store.put("roundtrip", bytes([0, 127, 255]))
        assert await store.get("roundtrip") == bytes([0, 127, 255])
        assert "roundtrip" in await store.keys()
        await store.flush()
        if config["scoped"]:
            scoped = await context.intercept("kv@1.0", {"namespace": "tenant"})
            view = await scoped.get("kv@1.0", bindings, bindings.KvClient)
            await view.put("roundtrip", b"private")
            assert await view.get("roundtrip") == b"private"
            assert await store.get("roundtrip") == bytes([0, 127, 255])

        async def body(scope):
            async def cleanup(_context):
                Path(config["cleanup"]).write_text("cleaned\n")

            await scope.defer(cleanup)

        effect = await context.effect(body, asynchronous=True)
        await effect.settled()
        assert await context.is_current()
        Path(config["result"]).write_text(
            json.dumps({"bytes": [0, 127, 255], "scoped": config["scoped"]})
        )

    run("python-probe", [Component("probe", apply, inject={"kv@1.0": True})])


def scenario(name, python_provider):
    project = EVIDENCE / (name + ("-" + sys.argv[1] if len(sys.argv) > 1 else ""))
    project.mkdir()
    plugins = project / "plugins"
    plugins.mkdir()
    python = ["conda", "run", "--no-capture-output", "-n", "base", "python"]
    manifest(
        plugins / "probe",
        "python-probe",
        python + [str(Path(__file__).resolve()), "probe"],
    )
    if python_provider:
        manifest(plugins / "store", "kv", python + ["-m", "kv_py"])
        shutil.copy2(EVIDENCE / "bin/libcounter.dylib", plugins / "libcounter.dylib")
        provider, counter = "kv", "counter"
    else:
        shutil.copy2(EVIDENCE / "bin/libkv_mem.dylib", plugins / "libkv_mem.dylib")
        manifest(plugins / "counter", "counter", python + ["-m", "counter_py"])
        provider, counter = "kv", "counter"
    configuration = (
        "[runtime]\ncheckpoint_ms=0\n"
        f'[instance.store]\nplugin="{provider}"\ncomponent="kv-store"\n'
        f'[instance.counter]\nplugin="{counter}"\ncomponent="counter"\nconfig={{interval_ms=20,key="count"}}\n'
        '[instance.probe]\nplugin="python-probe"\ncomponent="probe"\n'
        f"config={{scoped={str(python_provider).lower()},result={json.dumps(str(project / 'observed.json'))},cleanup={json.dumps(str(project / 'cleanup.txt'))}}}\n"
    )
    (project / "cordis.toml").write_text(configuration)
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
            deadline = time.monotonic() + 25
            while not (project / "observed.json").exists():
                if process.poll() is not None:
                    raise RuntimeError(f"{name}核心退出 {process.returncode}")
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"{name}观察结果超时")
                time.sleep(0.1)
            time.sleep(0.4)
            process.send_signal(signal.SIGTERM)
            code = process.wait(timeout=15)
            assert code == 0, f"核心有序退出：{code}"
            assert (project / "cleanup.txt").read_text() == "cleaned\n"
            return {
                "scenario": name,
                "exit": code,
                "observed": json.loads((project / "observed.json").read_text()),
                "cleanup": True,
            }
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "probe":
        probe()
    else:
        results = [
            scenario("python-provider", True),
            scenario("python-consumer", False),
        ]
        (EVIDENCE / "smoke-result.json").write_text(
            json.dumps(results, indent=2) + "\n"
        )
        print(json.dumps(results, indent=2))
