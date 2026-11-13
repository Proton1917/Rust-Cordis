import json
import shutil
import subprocess
import sys

from aggregate import manifest
from files import equal
from measure import fixture, runtime, save


def run(options):
    project = options.evidence / "streams"
    (project / "plugins").mkdir(parents=True)
    (project / "tmp").mkdir()
    source = project / "source.bin"
    with source.open("wb") as file:
        block = bytes(range(256)) * 4096
        for _ in range(64):
            file.write(block)
    shutil.copy2(options.tools / "libhttp_test_native.dylib", project / "plugins/libhttp_test_native.dylib")
    configuration = '[runtime]\ncheckpoint_ms=0\napply_timeout_ms=120000\n[instance.provider]\nplugin="libhttp_test_native.dylib"\ncomponent="stream-provider"\ngroup="provider"\nconfig={file=' + json.dumps(str(source)) + '}\n'
    for language in ("same", "rust", "python", "typescript"):
        output, result = project / (language + ".bin"), project / (language + ".json")
        if language in ("same", "rust"):
            plugin = "libhttp_test_native.dylib"
            group = "provider" if language == "same" else "rust"
            config = f'output={json.dumps(str(output))},result={json.dumps(str(result))},cancel=false,typed={str(language == "same").lower()}'
        else:
            command = [sys.executable, "-B", str(options.workspace / "tests/http-sdk/stream-receiver.py")] if language == "python" else ["node", str(options.workspace / "tests/http-sdk/process/dist/stream-receiver.js")]
            manifest(project / "plugins" / language, "stream-receiver-py" if language == "python" else "stream-receiver-ts", command)
            plugin, group = language, language
            config = f'output={json.dumps(str(output))},result={json.dumps(str(result))}'
        configuration += f'[instance.{language}]\nplugin={json.dumps(plugin)}\ncomponent="stream-probe"\ngroup={json.dumps(group)}\nconfig={{{config}}}\n'
    configuration += fixture(project, options.tools, "admin")
    (project / "cordis.toml").write_text(configuration)
    with runtime(project, options) as current:
        def finished():
            snapshot = current.status()
            if snapshot and (snapshot["instance_errors"] or snapshot["plugin_errors"]):
                raise RuntimeError(json.dumps(snapshot))
            return all((project / (name + ".json")).exists() for name in ("same", "rust", "python", "typescript"))
        current.wait(finished, "四种位置的通用流传输完成", seconds=120)
        results = {}
        for name in ("same", "rust", "python", "typescript"):
            equal(source, project / (name + ".bin"))
            results[name] = json.loads((project / (name + ".json")).read_text())
            assert results[name]["bytes"] == 67108864
        current.wait(lambda: (project / "admin-commands").is_dir(), "管理入口")
        cancelled = project / "cancelled.json"
        addition = f'[instance.cancelled]\nplugin="libhttp_test_native.dylib"\ncomponent="stream-probe"\ngroup="cancelled"\nconfig={{output={json.dumps(str(project / "partial.bin"))},result={json.dumps(str(cancelled))},cancel=true,typed=false}}\n'
        (project / "cordis.toml").write_text(configuration + addition)
        current.request("admin", "cordis.manage@1.1", "reload", [])
        current.wait(cancelled.exists, "中途释放流对象")
        report = json.loads(cancelled.read_text())
        assert report["cancelled"] and 0 < report["bytes"] <= 1048576
        current.request("admin", "cordis.manage@1.1", "remove", ["cancelled"])
        sender = next(group["pid"] for group in current.status()["groups"] if group["name"] == "provider")
        def released():
            output = subprocess.run(["lsof", "-a", "-p", str(sender), "-F", "n", "--", str(source)], capture_output=True, text=True)
            return output.returncode == 1 and str(source) not in output.stdout
        current.wait(released, "发送方文件句柄随流释放关闭", seconds=10)
        save(project / "observed-status.json", current.status())
    return {"transfers": results, "cancelled": report, "sender_pid": sender, "sender_file_closed": True, "source_bytes": 67108864}
