import json
import shutil
import subprocess

from support import EVIDENCE, manifest, test_environment


project = EVIDENCE / "kv-contract"
project.mkdir()
plugins = project / "plugins"
plugins.mkdir()
shutil.copy2(EVIDENCE / "bin/libkv_contract.dylib", plugins / "libkv_contract.dylib")
manifest(
    plugins / "kv-py",
    "kv",
    ["conda", "run", "--no-capture-output", "-n", "base", "python", "-m", "kv_py"],
)
(project / "cordis.toml").write_text(
    '[runtime]\ncheckpoint_ms=0\n[instance.store]\nplugin="kv"\ncomponent="kv-store"\n'
    '[instance.suite]\nplugin="kv-contract"\ncomponent="kv-contract"\n'
)
environment = test_environment()
with (
    (project / "stdout.log").open("w") as output,
    (project / "stderr.log").open("w") as error,
):
    result = subprocess.run(
        [
            str(EVIDENCE / "bin/cordis"),
            "run",
            "--contract-suite",
            "suite",
            "--contract-subject",
            "store",
            "--contract-report",
            str(project / "report.json"),
            "--contract-timeout-ms",
            "20000",
        ],
        cwd=project,
        env=environment,
        stdout=output,
        stderr=error,
        timeout=30,
    )
report = json.loads((project / "report.json").read_text())
assert result.returncode == 0, report
assert report["finished"] and report["passed"], report
print(json.dumps(report, indent=2))
