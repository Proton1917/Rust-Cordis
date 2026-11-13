import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = Path(os.environ["CORDIS_TEST_EVIDENCE"])
PLUGIN = ROOT / "tests/typescript-sdk/plugin/dist"
PROVIDER = ROOT / "plugins/kv-ts/dist/main.js"


def manifest(path, name, module):
    path.mkdir(parents=True)
    (path / "cordis-plugin.toml").write_text(
        f'name={json.dumps(name)}\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(["node", str(module)])}\n'
    )


def environment():
    result = os.environ.copy()
    result["PATH"] = str(EVIDENCE / "bin") + os.pathsep + result["PATH"]
    result["TMPDIR"] = str(EVIDENCE / "tmp")
    return result


def stop(process):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def smoke(label, provider):
    project = EVIDENCE / label
    project.mkdir()
    plugins = project / "plugins"
    manifest(plugins / "probe", "typescript-probe", PLUGIN / "probe.js")
    if provider:
        manifest(plugins / "store", "kv", PROVIDER)
        shutil.copy2(EVIDENCE / "bin/libcounter.dylib", plugins / "libcounter.dylib")
        store = "kv"
        counter = '[instance.counter]\nplugin="counter"\ncomponent="counter"\nconfig={interval_ms=20}\n'
    else:
        shutil.copy2(EVIDENCE / "bin/libkv_mem.dylib", plugins / "libkv_mem.dylib")
        store, counter = "kv", ""
    (project / "cordis.toml").write_text(
        f'[runtime]\ncheckpoint_ms=0\n[instance.store]\nplugin="{store}"\ncomponent="kv-store"\n'
        + counter + '[instance.probe]\nplugin="typescript-probe"\ncomponent="probe"\n'
        f'config={{scoped={str(provider).lower()},result={json.dumps(str(project / "observed.json"))},cleanup={json.dumps(str(project / "cleanup.txt"))}}}\n'
    )
    with (project / "stdout.log").open("w") as output, (project / "stderr.log").open("w") as error:
        process = subprocess.Popen([str(EVIDENCE / "bin/cordis"), "run"], cwd=project, env=environment(), stdout=output, stderr=error)
        try:
            deadline = time.monotonic() + 25
            while not (project / "observed.json").exists():
                if process.poll() is not None:
                    raise RuntimeError(f"{label}核心退出 {process.returncode}")
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"{label}观察结果超时")
                time.sleep(0.05)
            snapshot = json.loads((project / "state/status.json").read_text())
            (project / "active.json").write_text(json.dumps(snapshot, indent=2))
            stop(process)
            assert process.returncode == 0, process.returncode
            assert (project / "cleanup.txt").read_text() == "cleaned\n"
            result = {"scenario": label, "exit": process.returncode, "cleanup": True,
                      "observed": json.loads((project / "observed.json").read_text())}
            (project / "result.json").write_text(json.dumps(result, indent=2))
            print(json.dumps(result))
        finally:
            stop(process)


def contract(label):
    project = EVIDENCE / label
    project.mkdir()
    plugins = project / "plugins"
    manifest(plugins / "store", "kv", PROVIDER)
    shutil.copy2(EVIDENCE / "bin/libkv_contract.dylib", plugins / "libkv_contract.dylib")
    (project / "cordis.toml").write_text(
        '[runtime]\ncheckpoint_ms=0\n[instance.store]\nplugin="kv"\ncomponent="kv-store"\n'
        '[instance.suite]\nplugin="kv-contract"\ncomponent="kv-contract"\n'
    )
    with (project / "stdout.log").open("w") as output, (project / "stderr.log").open("w") as error:
        result = subprocess.run([
            str(EVIDENCE / "bin/cordis"), "run", "--contract-suite", "suite", "--contract-subject", "store",
            "--contract-report", str(project / "report.json"), "--contract-timeout-ms", "20000",
        ], cwd=project, env=environment(), stdout=output, stderr=error, timeout=30)
    report = json.loads((project / "report.json").read_text())
    assert result.returncode == 0 and report["finished"] and report["passed"], report
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    case = sys.argv[1]
    label = sys.argv[2] if len(sys.argv) > 2 else case
    if case == "contract":
        contract(label)
    else:
        smoke(label, case == "typescript-provider")
