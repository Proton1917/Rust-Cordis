import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time
import tomllib

import runtime_inputs


def command(arguments, directory, log, environment=None):
    with log.open("w") as output:
        subprocess.run(arguments, cwd=directory, env=environment, stdout=output, stderr=subprocess.STDOUT, check=True)


def save(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def prepare_node(options, project, environment):
    """M19/M21：SDK及真实KV插件在守卫目录从源码构建，发布副本保留生产依赖。"""
    build = options.evidence / "node-source"
    sdk = build / "sdks/typescript/cordis-sdk"
    plugin = build / "plugins/kv-ts"
    for source, destination in [
        (options.workspace / "sdks/typescript/cordis-sdk", sdk),
        (options.workspace / "plugins/kv-ts", plugin),
    ]:
        shutil.copytree(source, destination, ignore=shutil.ignore_patterns("node_modules", "dist", "__pycache__"))
    for name, directory in [("sdk", sdk), ("kv", plugin)]:
        command(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund", "--cache",
                 str(options.workspace / "target/npm-cache")], directory,
                options.evidence / f"node-{name}-dependencies.log", environment)
        command(["npm", "run", "build"], directory,
                options.evidence / f"node-{name}-build.log", environment)
    command(["npm", "prune", "--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund", "--cache",
             str(options.workspace / "target/npm-cache")], plugin,
            options.evidence / "node-production-dependencies.log", environment)
    shutil.copytree(plugin, project / "plugins/node", ignore=shutil.ignore_patterns(
        "__pycache__", "src", "tsconfig.json",
    ))


def prepare(options):
    """DOC-15 §12.2：真实脚本实现、SDK和生产依赖进入本次发布项目。"""
    project = options.evidence / "project"
    plugins = project / "plugins"
    plugins.mkdir(parents=True)
    (options.evidence / "tmp").mkdir()
    python = plugins / "python"
    shutil.copytree(options.workspace / "plugins/kv-py", python, ignore=shutil.ignore_patterns("__pycache__"))
    shutil.copytree(options.workspace / "sdks/python/cordis_sdk", python / "cordis_sdk", ignore=shutil.ignore_patterns("__pycache__"))
    shutil.copytree(options.python_dependencies, python, dirs_exist_ok=True, ignore=shutil.ignore_patterns("__pycache__"))
    environment = os.environ.copy()
    environment["TMPDIR"] = str(options.evidence / "tmp")
    prepare_node(options, project, environment)
    client = plugins / "reader"
    client.mkdir()
    shutil.copy2(options.tools / "cordis-runtime-fixture", client / "reader")
    (client / "cordis-plugin.toml").write_text('name="fixture-kv-client"\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand=["./reader","kv-client","v1"]\n')
    configuration = f'[pack]\nname="script-runtimes"\nversion="0.1.0"\nruntimes=["python","node"]\n[pack.runtime_paths]\npython={json.dumps(str(options.runtimes / "python"))}\nnode={json.dumps(str(options.runtimes / "node"))}\n[runtime]\ncheckpoint_ms=0\nrestart_max=0\n'
    for language in ("python", "node"):
        configuration += f'[scope.{language}]\nisolate={{"kv@1"="{language}"}}\n[instance.{language}]\nplugin="{language}"\ncomponent="kv-store"\ngroup="{language}"\nscope="{language}"\n'
        configuration += f'[instance.{language}-reader]\nplugin="reader"\ncomponent="kv-client"\ngroup="{language}-reader"\nscope="{language}"\nconfig={{commands="../../commands/{language}",results="../../results/{language}"}}\n'
    (project / "cordis.toml").write_text(configuration)
    return project


def unpack(options, project):
    environment = os.environ.copy()
    environment["TMPDIR"] = str(options.evidence / "tmp")
    command([str(options.dev), "pack", "--project", str(project), "--target", "aarch64-apple-darwin", "--runtime-dir", str(options.tools), "--out", "dist", "--sign=-"], project, options.evidence / "pack.log", environment)
    archive = next((project / "dist").glob("*.tar.gz"))
    relocated = options.evidence / "relocated"
    relocated.mkdir()
    command(["/usr/bin/tar", "-xzf", str(archive), "-C", str(relocated)], project, options.evidence / "extract.log")
    return next(relocated.iterdir())


def package_metadata(options, deployed):
    """AU-12：入口、许可证和动态扩展均从移动后的包取得。"""
    python = deployed / "runtimes/python/bin/python3"
    node = deployed / "runtimes/node/bin/node"
    assert python.resolve().is_relative_to(deployed)
    assert node.resolve().is_relative_to(deployed)
    assert (deployed / "runtimes/python/lib/python3.12/LICENSE.txt").is_file()
    assert (deployed / "runtimes/node/LICENSE").is_file()
    configuration = tomllib.loads((deployed / "cordis.toml").read_text())
    packages = {name: deployed / "plugins" / configuration["instance"][name]["plugin"] for name in ("python", "node")}
    for name, path in packages.items():
        manifest = tomllib.loads((path / "cordis-plugin.toml").read_text())
        expected = python if name == "python" else node
        assert (path / manifest["command"][0]).resolve() == expected.resolve()
    environment = isolated_environment(deployed, options.evidence)
    scripts = Path(__file__).parent / "probes"
    command([str(python), "-I", "-B", str(scripts / "python.py"), str(packages["python"])], deployed, options.evidence / "python-runtime.json", environment)
    command([str(node), str(scripts / "node.cjs"), str(packages["node"])], deployed, options.evidence / "node-runtime.json", environment)
    py = json.loads((options.evidence / "python-runtime.json").read_text())
    js = json.loads((options.evidence / "node-runtime.json").read_text())
    for path in (py["executable"], py["prefix"], py["extension"], js["executable"], js["native"]):
        assert Path(path).resolve().is_relative_to(deployed), path
    assert js["dynamic_code"] == 42
    return {"python": py, "node": js}


def isolated_environment(deployed, evidence):
    environment = os.environ.copy()
    for name in ("PYTHONPATH", "PYTHONHOME", "NODE_PATH", "NODE_OPTIONS"):
        environment.pop(name, None)
    environment["PATH"] = str(deployed) + ":/usr/bin:/bin"
    environment["TMPDIR"] = str(evidence / "tmp")
    return environment


def wait(process, observe, description):
    deadline = time.monotonic() + 35
    while time.monotonic() < deadline:
        assert process.poll() is None, f"核心提前退出{process.returncode}"
        value = observe()
        if value:
            return value
        time.sleep(0.02)
    raise TimeoutError(description)


def invoke(process, deployed, role, number, method, arguments):
    path = deployed / "commands" / role / f"{number}.json"
    staged = path.with_suffix(".next")
    save(staged, {"interface": "kv@1.0", "method": method, "args": arguments})
    staged.rename(path)
    result = deployed / "results" / role / path.name
    reply = wait(process, lambda: json.loads(result.read_text()) if result.exists() else None, method)
    assert reply["ok"], reply
    return reply["value"]


def run(options, deployed):
    """AU-12：两个独立解释器经UP/1接受真实读写，随后核心有序关闭全部进程。"""
    environment = isolated_environment(deployed, options.evidence)
    with (options.evidence / "core.stdout.log").open("w") as output, (options.evidence / "core.stderr.log").open("w") as error:
        process = subprocess.Popen([str(deployed / "cordis"), "run"], cwd=deployed, env=environment, stdout=output, stderr=error)
    try:
        def active():
            path = deployed / "state/status.json"
            if path.exists():
                snapshot = json.loads(path.read_text())
                if len(snapshot["instances"]) == 4 and all(item["state"] == "active" for item in snapshot["instances"]):
                    return snapshot
            return None

        snapshot = wait(process, active, "两种真实脚本插件与使用方Active")
        values = {"python": [0, 127, 255, 12], "node": [1, 128, 254, 22]}
        for role, value in values.items():
            invoke(process, deployed, role, 1, "put", ["shared-key", value])
            assert invoke(process, deployed, role, 2, "get", ["shared-key"]) == value
            assert invoke(process, deployed, role, 3, "keys", []) == ["shared-key"]
            invoke(process, deployed, role, 4, "flush", [])
        save(options.evidence / "active.json", snapshot)
        pids = [group["pid"] for group in snapshot["groups"]]
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
        try:
            code = process.wait(timeout=40)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise
        save(options.evidence / "exit.json", {"core": process.pid, "exit": code})
    assert code == 0
    for pid in pids:
        checked = subprocess.run(["/bin/ps", "-p", str(pid), "-o", "pid="], capture_output=True, text=True)
        assert checked.returncode == 1 and not checked.stdout.strip(), checked.stdout
    return {"values": values, "child_pids": pids, "core_exit": code}


def main():
    parser = argparse.ArgumentParser()
    for name in ("workspace", "evidence", "tools", "dev", "python-dependencies"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--runtimes", type=Path)
    options = parser.parse_args()
    for name, value in vars(options).items():
        if value is not None:
            setattr(options, name, value.resolve())
    options.evidence.mkdir(parents=True)
    options.runtimes = runtime_inputs.prepare(options)
    project = prepare(options)
    deployed = unpack(options, project)
    metadata = package_metadata(options, deployed)
    behavior = run(options, deployed)
    save(options.evidence / "result.json", {"deployed": str(deployed), "metadata": metadata, "behavior": behavior})


if __name__ == "__main__":
    main()
