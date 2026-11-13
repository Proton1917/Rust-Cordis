import json
import signal
import subprocess
import time

from support import EVIDENCE, manifest, test_environment


def execute():
    project = EVIDENCE / "named-realms"
    project.mkdir()
    plugins = project / "plugins"
    plugins.mkdir()
    manifest(
        plugins / "kv-python",
        "kv",
        ["conda", "run", "--no-capture-output", "-n", "base", "python", "-m", "kv_py"],
    )
    manifest(
        plugins / "client",
        "fixture-kv-client",
        [str(EVIDENCE / "bin/cordis-runtime-fixture"), "kv-client", "v1"],
    )
    config = "[runtime]\ncheckpoint_ms=0\n"
    for name in ("a", "b"):
        config += f'[instance.store-{name}]\nplugin="kv"\ncomponent="kv-store"\nisolate={{"kv@1"="tenant-{name}"}}\n'
        config += f'[instance.client-{name}]\nplugin="fixture-kv-client"\ncomponent="kv-client"\nisolate={{"kv@1"="tenant-{name}"}}\n'
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

            def wait_for(operation):
                deadline = time.monotonic() + 20
                while True:
                    value = operation()
                    if value is not None:
                        return value
                    if process.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError("命名域场景超时")
                    time.sleep(0.02)

            for name in ("a", "b"):
                wait_for(
                    lambda: True if (project / (name + "-commands")).exists() else None
                )

            def command(name, serial, method, args):
                path = project / (name + "-commands") / f"{serial}.json"
                staged = path.with_suffix(".next")
                staged.write_text(
                    json.dumps({"interface": "kv@1.0", "method": method, "args": args})
                )
                staged.rename(path)
                response = project / (name + "-results") / path.name
                result = wait_for(
                    lambda: (
                        json.loads(response.read_text()) if response.exists() else None
                    )
                )
                assert result["ok"], result
                return result["value"]

            command("a", 1, "put", ["shared", [1, 2]])
            command("b", 1, "put", ["shared", [9, 8]])
            first = command("a", 2, "get", ["shared"])
            second = command("b", 2, "get", ["shared"])
            assert first == [1, 2] and second == [9, 8]
            process.send_signal(signal.SIGTERM)
            assert process.wait(timeout=15) == 0
            result = {"tenant-a": first, "tenant-b": second, "exit": 0}
            (project / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result))
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


execute()
