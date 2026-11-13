import json
import os
from pathlib import Path
import shutil
import subprocess

from measure import runtime, save


def command(arguments, directory, log, environment=None):
    result = subprocess.run(arguments, cwd=directory, env=environment, capture_output=True, text=True)
    log.write_text(result.stdout + result.stderr)
    assert result.returncode == 0, result.stderr


def run(options):
    project = options.evidence / "project"
    root = options.workspace
    tool = str(options.tools / "cordis-dev")
    command([tool, "new", str(project), "--sdk-path", str(root / "crates/cordis-sdk")], root, options.evidence / "new.log")
    for name, language in (("native", "--rust"), ("python", "--python"), ("node", "--ts")):
        command([tool, "new-plugin", name, language, "--http", "--project", str(project)], root, options.evidence / (name + "-create.log"))
    environment = os.environ.copy()
    environment.pop("CARGO_TARGET_DIR", None)
    command(["cargo", "build", "--target-dir", str(project / "target")], project, options.evidence / "rust-build.log", environment)
    shutil.copytree(options.dependencies, project / "plugins/python/.dependencies")
    environment["npm_config_cache"] = str(root / "target/npm-cache")
    node = project / "plugins/node"
    command(["npm", "install", "--ignore-scripts", "--no-audit", "--no-fund"], node, options.evidence / "node-install.log", environment)
    command(["npm", "run", "build"], node, options.evidence / "node-build.log", environment)
    shutil.copy2(options.tools / "libcordis_http.dylib", project / "target/debug/libcordis_http.dylib")
    with (project / "cordis.toml").open("a") as output:
        output.write('\n[instance.http]\nplugin="http"\ncomponent="http"\n')
    (project / "tmp").mkdir()
    with runtime(project, options) as current:
        current.wait(lambda: all(current.active(name) for name in ("app", "watch", "native", "python", "node", "http")), "三个生成的 HTTP 处理方激活", seconds=60)
        address = project / "state/data/http/listeners/http.json"
        current.wait(address.exists, "HTTP 地址")
        host, port = json.loads(address.read_text())["bound"].rsplit(":", 1)
        import http.client
        results = {}
        for name in ("native", "python", "node"):
            connection = http.client.HTTPConnection(host, int(port), timeout=10)
            try:
                connection.request("GET", "/" + name)
                response = connection.getresponse()
                body = response.read().decode()
                assert response.status == 200 and body == f"GET /{name}\n", (name, response.status, body)
                results[name] = {"status": response.status, "body": body}
            finally:
                connection.close()
        save(project / "observed-status.json", current.status())
    shutil.rmtree(project / "target")
    return results
