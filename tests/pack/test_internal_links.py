"""Run pack -> extract -> core startup -> restart for internal file/directory links.

Arguments point at binaries built from the same checkout. The fixture is
crates/cordis-runtime/fixtures/plugin (cordis-runtime-fixture).
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tarfile
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("--tools", type=Path, required=True)
parser.add_argument("--fixture", type=Path, required=True)
parser.add_argument("--dev", type=Path, required=True)
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix="cordis-pack-links-") as temporary:
    project = Path(temporary)
    plugin = project / "plugins/store"
    (plugin / "bin").mkdir(parents=True)
    (plugin / "data").mkdir()
    shutil.copy2(args.fixture, plugin / "bin/fixture")
    (plugin / "run").symlink_to("bin/fixture")
    (plugin / "data/value").write_text("artifact-value")
    (plugin / "resources").symlink_to("data", target_is_directory=True)
    (plugin / "cordis-plugin.toml").write_text(
        "name='fixture-store'\nversion='0.1.0'\nsocket='process'\nprotocol=1\ncommand=['./run','store','v1']\n"
    )
    (project / "cordis.toml").write_text(
        "format_version=2\n[pack]\nname='linked-plugin'\nversion='0.1.0'\n[runtime]\ncheckpoint_ms=0\n"
        "[instance.store]\nplugin='store'\nconfig={initial=41}\n"
    )
    target = "aarch64-apple-darwin" if os.uname().sysname == "Darwin" else "x86_64-unknown-linux-gnu"
    subprocess.run([
        str(args.dev), "pack", "--project", str(project), "--target", target,
        "--runtime-dir", str(args.tools), "--allow-private-paths",
    ], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=90)
    archive = next((project / "dist").glob("*.tar.gz"))
    with tarfile.open(archive) as package:
        package.extractall(project / "delivered", filter="data")
    deployed = next((project / "delivered").iterdir())
    observed = []
    for round_ in range(2):
        with (project / f"core-{round_}.log").open("w") as log:
            child = subprocess.Popen([str(deployed / "cordis"), "run", "--config", str(deployed / "cordis.toml")],
                                     cwd=deployed, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 15
                while True:
                    if child.poll() is not None:
                        raise AssertionError(f"core exited {child.returncode}: {(project / f'core-{round_}.log').read_text()}")
                    try:
                        state = json.loads((deployed / "state/status.json").read_text())
                        instance = next(item for item in state["instances"] if item["name"] == "store")
                        if instance["state"] == "active":
                            break
                    except (FileNotFoundError, StopIteration, json.JSONDecodeError):
                        pass
                    if time.monotonic() >= deadline:
                        raise AssertionError("linked plugin activation deadline")
                    time.sleep(.02)
                reference = instance["reference"]
                installed = deployed / "plugins" / reference
                assert (installed / "run").is_symlink()
                assert (installed / "resources").is_symlink()
                assert (installed / "resources/value").read_text() == "artifact-value"
                observed.append(reference)
            finally:
                if child.poll() is None:
                    child.send_signal(signal.SIGTERM)
                    try:
                        child.wait(timeout=8)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
                assert child.returncode == 0, child.returncode
    assert observed[0] == observed[1], observed
    print(json.dumps({"passed": True, "references": observed, "core_starts": 2, "orderly_exits": 2}))
