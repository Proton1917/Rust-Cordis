import argparse
import json
from pathlib import Path
import shutil
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmarks/multilanguage"))
from measure import fixture, runtime, save


def provider(options, plugins, language):
    if language in ("same", "rust"):
        shutil.copy2(options.tools / "libkv_mem.dylib", plugins / "libkv_mem.dylib")
        return "libkv_mem.dylib"
    directory = plugins / language
    directory.mkdir()
    command = [sys.executable, "-B", "-m", "kv_py"] if language == "python" else ["node", str(options.workspace / "plugins/kv-ts/dist/main.js")]
    (directory / "cordis-plugin.toml").write_text(f'name="kv"\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n')
    return language


def prepare(options, language, provider_language=None):
    project = options.evidence / language
    plugins = project / "plugins"
    plugins.mkdir(parents=True)
    (project / "tmp").mkdir()
    shutil.copy2(options.extra / "libdynamic_test_native.dylib", plugins / "libdynamic_test_native.dylib")
    shutil.copy2(options.extra / "libcordis_secrets.dylib", plugins / "libcordis_secrets.dylib")
    (project / "secrets.env").write_text("TOKEN=dynamic-test-value\n")
    (project / "secrets.env").chmod(0o600)
    provider_language = provider_language or language
    source = provider(options, plugins, provider_language)
    (project / "source.bin").write_bytes(bytes(range(256)) * 4096)
    shared = provider_language == "same"
    configuration = '[runtime]\ncheckpoint_ms=0\nconfirm_ms=20\napply_timeout_ms=30000\n'
    configuration += f'[instance.store]\nplugin={json.dumps(source)}\ncomponent="kv-store"\ngroup={json.dumps("shared" if shared else "store")}\n'
    if options.scenario == "lifecycle":
        configuration += 'config={path="dynamic.cbor"}\n'
    configuration += f'[instance.identity]\nplugin="libdynamic_test_native.dylib"\ncomponent="identity"\ngroup={json.dumps("shared" if shared else "identity")}\n'
    configuration += f'[instance.streams]\nplugin="libdynamic_test_native.dylib"\ncomponent="streams"\ngroup="streams"\nconfig={{file={json.dumps(str(project / "source.bin"))}}}\n'
    configuration += '[instance.secrets]\nplugin="libcordis_secrets.dylib"\ncomponent="secrets"\nconfig={file="secrets.env",grants={probe=["TOKEN"]}}\n'
    probe = f'[instance.probe]\nplugin="libdynamic_test_native.dylib"\ncomponent="probe"\ngroup={json.dumps("shared" if shared else "probe")}\nconfig={{result={json.dumps(str(project / "probe.json"))},stream=true}}\n'
    configuration += fixture(project, options.tools, "admin")
    (project / "cordis.toml").write_text(configuration)
    return project, probe


def observed(current, project, previous=None):
    status = current.status()
    if status is None:
        return None
    probe = next((item for item in status["instances"] if item["name"] == "probe"), None)
    if probe and probe["state"] == "failed":
        raise RuntimeError(json.dumps(status))
    result = project / "probe.json"
    if probe and probe["state"] == "active" and result.exists():
        record = json.loads(result.read_text())
        if record["context"] == probe["context"] and record["context"] != previous and record.get("iterations", 0) >= 10:
            return record
    return None


def manage(current, method, arguments, success=True):
    return current.request("admin", "cordis.manage@1.1", method, arguments, expect_success=success)[0]


def lifecycle(current, project, options, original):
    sources = project / "sources"
    sources.mkdir()
    for name in ("libkv_file.dylib", "libkv_evolution.dylib"):
        shutil.copy2(options.extra / name, sources / name)
    accepted = manage(current, "install", [str(sources / "libkv_file.dylib")])
    manage(current, "replace", ["store", accepted["value"]])
    replaced = current.wait(lambda: observed(current, project, original["context"]), "动态调用随文件提供方重建")
    assert (project / "state/data/kv/dynamic.cbor").is_file()
    assert replaced["metadata"]["cordis.caller"] == {"instance": "probe"}
    candidate = manage(current, "install", [str(sources / "libkv_evolution.dylib")])
    failure = manage(current, "replace", ["store", candidate["value"]], False)
    assert failure["ok"] is False
    restored = current.wait(lambda: observed(current, project, replaced["context"]), "失败候选回滚后继续两种调用")
    snapshot = current.status()
    store = next(item for item in snapshot["instances"] if item["name"] == "store")
    assert store["reference"] == accepted["value"]
    manage(current, "remove", ["store"])
    def pending():
        state = current.status()
        probe = next(item for item in state["instances"] if item["name"] == "probe")
        return state if probe["state"] == "pending" else None
    removed = current.wait(pending, "提供方卸下后动态使用方 Pending")
    return {"before": original, "replaced": replaced, "failure": failure, "restored": restored, "removed": removed}


def scripts(options):
    results = {}
    for language in ["python", "typescript"]:
        project, _ = prepare(options, "script-" + language, "rust")
        directory = project / "plugins/script"
        directory.mkdir()
        command = [sys.executable, "-B", str(options.workspace / "tests/dynamic-sdk/python_client.py")] if language == "python" else ["node", str(options.workspace / "tests/dynamic-sdk/script/dist/main.js")]
        (directory / "cordis-plugin.toml").write_text(f'name="dynamic-script-tests"\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n')
        result = project / "script-result.json"
        with (project / "cordis.toml").open("a") as configuration:
            configuration.write(f'[instance.script]\nplugin="script"\ncomponent="probe"\nconfig={{result={json.dumps(str(result))}}}\n')
        with runtime(project, options, python_paths=[options.workspace / "plugins/kv-py"]) as current:
            def ready():
                status = current.status()
                if status:
                    instance = next((item for item in status["instances"] if item["name"] == "script"), None)
                    if instance and instance["state"] == "failed":
                        raise RuntimeError(json.dumps(instance))
                    return instance and instance["state"] == "active" and result.exists()
                return False
            current.wait(ready, "脚本动态调用及资源参数拒绝")
            results[language] = json.loads(result.read_text())
            save(project / "observed-status.json", current.status())
    return results


def main():
    parser = argparse.ArgumentParser()
    for name in ("workspace", "tools", "extra", "dependencies", "evidence"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--scenario", choices=["matrix", "lifecycle", "scripts"], required=True)
    options = parser.parse_args()
    for name in ("workspace", "tools", "extra", "dependencies", "evidence"):
        setattr(options, name, getattr(options, name).resolve())
    results = {}
    if options.scenario == "scripts":
        save(options.evidence / "result.json", {"scenario": "scripts", "result": scripts(options), "exit": 0})
        return
    for language in ["same", "rust", "python", "typescript"] if options.scenario == "matrix" else ["rust"]:
        project, probe = prepare(options, language)
        with runtime(project, options, python_paths=[options.workspace / "plugins/kv-py"]) as current:
            current.wait(lambda: current.active("secrets") and (project / "admin-commands").is_dir(), "实际密钥提供方与管理入口就绪")
            with (project / "cordis.toml").open("a") as configuration:
                configuration.write(probe)
            manage(current, "reload", [])
            original = current.wait(lambda: observed(current, project), "类型化与动态调用完成真实比较")
            assert original["typed"] == (language == "same")
            assert original["metadata"]["cordis.caller"] == {"instance": "probe"}
            results[language] = lifecycle(current, project, options, original) if options.scenario == "lifecycle" else original
            save(project / "observed-status.json", current.status())
        print("通过 " + language, flush=True)
    save(options.evidence / "result.json", {"scenario": options.scenario, "result": results, "exit": 0})


if __name__ == "__main__":
    main()
