import json
from pathlib import Path
import shutil
import sys

from measure import fixture, runtime, save


def manifest(directory, name, command):
    directory.mkdir(parents=True)
    (directory / "cordis-plugin.toml").write_text(f'name={json.dumps(name)}\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n')


def project(options, name):
    directory = options.evidence / name
    (directory / "plugins").mkdir(parents=True)
    (directory / "tmp").mkdir()
    return directory


def exchange(options):
    directory = project(options, "aggregate")
    plugins = directory / "plugins"
    shutil.copy2(options.tools / "libhttp_test_native.dylib", plugins / "libhttp_test_native.dylib")
    configuration = '[runtime]\ncheckpoint_ms=0\napply_timeout_ms=120000\n'
    for language in ("rust", "python", "typescript"):
        configuration += f'[scope.{language}]\nisolate={{"test.byte-payload@1"="{language}"}}\n'
        if language == "rust":
            reference = "libhttp_test_native.dylib"
        else:
            command = [sys.executable, "-B", str(options.workspace / "tests/http-sdk/bytes-provider.py")] if language == "python" else ["node", str(options.workspace / "tests/http-sdk/process/dist/bytes-provider.js")]
            name = "byte-provider-py" if language == "python" else "byte-provider-ts"
            manifest(plugins / language, name, command)
            reference = language
        configuration += f'[instance.{language}]\nplugin={json.dumps(reference)}\ncomponent="bytes-provider"\ngroup={json.dumps(language)}\nscope={json.dumps(language)}\n'
        configuration += f'[instance.{language}-probe]\nplugin="libhttp_test_native.dylib"\ncomponent="bytes-probe"\ngroup={json.dumps(language + "-probe")}\nscope={json.dumps(language)}\nconfig={{result={json.dumps(str(directory / (language + ".json")))}}}\n'
    (directory / "cordis.toml").write_text(configuration)
    with runtime(directory, options) as current:
        def ready():
            snapshot = current.status()
            if snapshot and (snapshot["instance_errors"] or snapshot["plugin_errors"]):
                raise RuntimeError(json.dumps(snapshot))
            return all((directory / (language + ".json")).exists() and current.active(language + "-probe") for language in ("rust", "python", "typescript"))
        current.wait(ready, "三种语言的五个 1 MiB 聚合字段往返")
        result = {language: json.loads((directory / (language + ".json")).read_text()) for language in ("rust", "python", "typescript")}
        save(directory / "observed-status.json", current.status())
    return result


def legacy(options):
    observed = {}
    for language in ("rust", "python", "typescript"):
        directory = project(options, "legacy-" + language)
        plugins = directory / "plugins"
        (directory / "state").mkdir()
        shutil.copy2(options.workspace / "tests/http-sdk/fixtures/m21-kv.state", directory / "state/store.state")
        if language == "rust":
            shutil.copy2(options.tools / "libkv_mem.dylib", plugins / "libkv_mem.dylib")
            reference = "libkv_mem.dylib"
        else:
            command = [sys.executable, "-B", "-m", "kv_py"] if language == "python" else ["node", str(options.workspace / "plugins/kv-ts/dist/main.js")]
            manifest(plugins / language, "kv", command)
            reference = language
        configuration = f'[runtime]\ncheckpoint_ms=0\n[instance.store]\nplugin={json.dumps(reference)}\ncomponent="kv-store"\ngroup="store"\n'
        configuration += fixture(directory, options.tools, "kv-client")
        (directory / "cordis.toml").write_text(configuration)
        with runtime(directory, options, [options.workspace / "plugins/kv-py"]) as current:
            current.wait(lambda: current.active("store") and current.active("kv-client") and (directory / "kv-client-commands").is_dir(), "M21 检查点恢复")
            reply, _ = current.request("kv-client", "kv@1.0", "get", ["count"])
            assert reply["value"] == [95, 0, 0, 0, 0, 0, 0, 0], reply
            observed[language] = reply
            save(directory / "observed-status.json", current.status())
    return observed


def run(options):
    return {"aggregate": exchange(options), "m21_checkpoint": legacy(options)}
