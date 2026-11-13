import json
import subprocess
import sys
import time

from acceptance import EVIDENCE, PLUGIN, environment, manifest, stop


project = EVIDENCE / (sys.argv[1] if len(sys.argv) > 1 else "resources")
project.mkdir()
manifest(project / "plugins/resources", "typescript-resources", PLUGIN / "resources.js")
(project / "cordis.toml").write_text(
    '[runtime]\ncheckpoint_ms=0\n[instance.parent]\nplugin="typescript-resources"\ncomponent="parent"\n'
    f'config={{evidence={json.dumps(str(project))}}}\n'
)
with (project / "stdout.log").open("w") as output, (project / "stderr.log").open("w") as error:
    process = subprocess.Popen([str(EVIDENCE / "bin/cordis"), "run"], cwd=project, env=environment(), stdout=output, stderr=error)
    try:
        deadline = time.monotonic() + 25
        while not (project / "observed.json").exists():
            if (project / "failure.json").exists():
                raise RuntimeError((project / "failure.json").read_text())
            if process.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("资源场景观察结果缺失")
            time.sleep(0.05)
        stop(process)
        assert process.returncode == 0, process.returncode
        assert (project / "cancelled.txt").read_text() == "cancelled\n"
        assert (project / "child-cleanup.txt").read_text() == "cleaned\n"
        print((project / "observed.json").read_text())
    finally:
        stop(process)
