import argparse
import json
from pathlib import Path
import shutil
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmarks/multilanguage"))
from measure import fixture, runtime, save


def library(name):
    return "lib" + name + (".dylib" if sys.platform == "darwin" else ".so")


def manifest(path, name, command):
    path.mkdir(parents=True)
    (path / "cordis-plugin.toml").write_text(
        f'name={json.dumps(name)}\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n'
    )


def project(options, name):
    directory = options.evidence / name
    (directory / "plugins").mkdir(parents=True)
    (directory / "tmp").mkdir()
    return directory


def provider(options, path, language):
    if language == "rust":
        name = library("kv_mem")
        shutil.copy2(options.tools / name, path / name)
        return name
    directory = path / language
    command = [sys.executable, "-B", "-m", "kv_py"] if language == "python" else ["node", str(options.workspace / "plugins/kv-ts/dist/main.js")]
    manifest(directory, "kv", command)
    return language


def active(current, count):
    snapshot = current.status()
    if snapshot and len(snapshot["instances"]) == count and all(item["state"] == "active" for item in snapshot["instances"]):
        return snapshot
    return None


def cross(options):
    """AU-3/4：三个真实提供方同核共存，已有probe补齐两个脚本语言方向。"""
    directory = project(options, "process-cross")
    plugins = directory / "plugins"
    configuration = '[runtime]\ncheckpoint_ms=0\nrestart_max=0\n'
    for language in ("rust", "python", "typescript"):
        reference = provider(options, plugins, language)
        configuration += f'[scope.{language}]\nisolate={{"kv@1"="{language}"}}\n[instance.{language}]\nplugin={json.dumps(reference)}\ncomponent="kv-store"\ngroup="{language}"\nscope="{language}"\n'
    commands = {
        "python-to-typescript": ("python-probe", "typescript", [sys.executable, "-B", str(options.workspace / "tests/python-sdk/smoke.py"), "probe"]),
        "typescript-to-python": ("typescript-probe", "python", ["node", str(options.workspace / "tests/typescript-sdk/plugin/dist/probe.js")]),
    }
    for label, (name, target, command) in commands.items():
        manifest(plugins / label, name, command)
        configuration += f'[instance.{label}]\nplugin="{label}"\ncomponent="probe"\ngroup="{label}"\nscope="{target}"\nconfig={{scoped=false,result={json.dumps(str(directory / (label + ".json")))},cleanup={json.dumps(str(directory / (label + "-cleanup.txt")))}}}\n'
    (directory / "cordis.toml").write_text(configuration)
    with runtime(directory, options, [options.workspace / "plugins/kv-py"]) as current:
        current.wait(lambda: all((directory / (name + ".json")).exists() for name in commands), "两个新方向完成真实读写")
        snapshot = current.wait(lambda: active(current, 5), "三语言及两个使用方Active")
        sources = {source["reference"]: source for source in snapshot["sources"]}
        providers = {}
        for language in ("rust", "python", "typescript"):
            item = next(item for item in snapshot["instances"] if item["name"] == language)
            source = sources[item["reference"]]
            assert item["group"] == language
            assert source["entry"]["socket"] == ("native" if language == "rust" else "process")
            providers[language] = {"instance": item, "source": source}
        observations = {name: json.loads((directory / (name + ".json")).read_text()) for name in commands}
        assert all(value["bytes"] == [0, 127, 255] for value in observations.values())
        save(directory / "active.json", snapshot)
    for name in commands:
        assert (directory / (name + "-cleanup.txt")).read_text() == "cleaned\n"
    result = {"core_pid": snapshot["pid"], "providers": providers, "directions": observations, "cleanup": True, "exit": 0}
    save(directory / "result.json", result)
    return result


def counter(current):
    reply, _ = current.request("kv-client", "kv@1.0", "get", ["count"])
    return int.from_bytes(bytes(reply["value"]), "little")


def migrated(current, previous, reference):
    snapshot = active(current, 4)
    if snapshot:
        items = {item["name"]: item for item in snapshot["instances"]}
        if items["store"]["reference"] == reference and items["counter"]["context"] != previous:
            return snapshot
    return None


def migration(options):
    """AU-5：同一Rust计数器持续写入，提供方按Rust、Python、TypeScript替换。"""
    directory = project(options, "three-language-migration")
    plugins = directory / "plugins"
    provider(options, plugins, "rust")
    counter_library = library("counter")
    shutil.copy2(options.tools / counter_library, plugins / counter_library)
    sources = directory / "sources"
    sources.mkdir()
    candidates = [sources / provider(options, sources, language) for language in ("python", "typescript")]
    configuration = f'[runtime]\ncheckpoint_ms=0\nconfirm_ms=20\nrestart_max=0\n[instance.store]\nplugin={json.dumps(library("kv_mem"))}\ncomponent="kv-store"\ngroup="store"\n[instance.counter]\nplugin={json.dumps(counter_library)}\ncomponent="counter"\ngroup="counter"\nconfig={{interval_ms=10,key="count"}}\n'
    configuration += fixture(directory, options.tools, "admin") + fixture(directory, options.tools, "kv-client")
    (directory / "cordis.toml").write_text(configuration)
    with runtime(directory, options, [options.workspace / "plugins/kv-py"]) as current:
        initial = current.wait(lambda: active(current, 4), "计数器与提供方Active")
        values = [current.wait(lambda: value if (value := counter(current)) > 0 else None, "Rust计数开始")]
        epochs = []
        replies = []
        snapshots = [initial]
        for candidate in candidates:
            before = current.wait(lambda: active(current, 4), "当前提供方稳定")
            prior = next(item["context"] for item in before["instances"] if item["name"] == "counter")
            previous_value = counter(current)
            installed, _ = current.request("admin", "cordis.manage@1.1", "install", [str(candidate)])
            replacement, _ = current.request("admin", "cordis.manage@1.1", "replace", ["store", installed["value"]])
            after = current.wait(lambda: migrated(current, prior, installed["value"]), "候选发布并重建计数器")
            observed = counter(current)
            assert observed >= previous_value, [previous_value, observed]
            advanced = current.wait(lambda: value if (value := counter(current)) > observed else None, "替换后继续写入")
            values.extend([observed, advanced])
            epochs.append([prior, next(item["context"] for item in after["instances"] if item["name"] == "counter")])
            replies.append(replacement)
            snapshots.append(after)
        result = {"sequence": ["Rust", "Python", "TypeScript"], "counts": values, "counter_contexts": epochs, "replacements": replies, "snapshots": snapshots, "core_pid": initial["pid"]}
    result["exit"] = 0
    save(directory / "result.json", result)
    return result


def contracts(options):
    """AU-6：原kv-contract按两阶段分别执行三种真实提供方的8项业务断言。"""
    results = {}
    for language in ("rust", "python", "typescript"):
        directory = project(options, "contract-" + language)
        reference = provider(options, directory / "plugins", language)
        contract_library = library("kv_contract")
        shutil.copy2(options.tools / contract_library, directory / "plugins" / contract_library)
        (directory / "cordis.toml").write_text(f'[runtime]\ncheckpoint_ms=0\n[instance.store]\nplugin={json.dumps(reference)}\ncomponent="kv-store"\ngroup="store"\n[instance.suite]\nplugin={json.dumps(contract_library)}\ncomponent="kv-contract"\ngroup="suite"\n')
        report = directory / "report.json"
        arguments = ["--contract-suite", "suite", "--contract-subject", "store", "--contract-report", str(report), "--contract-timeout-ms", "20000"]
        with runtime(directory, options, [options.workspace / "plugins/kv-py"], arguments) as current:
            code = current.process.wait(timeout=35)
            observed = json.loads(report.read_text())
            assert code == 0 and observed["finished"] and observed["passed"], observed
            assert len(observed["assertions"]) == 8 and all(item["passed"] for item in observed["assertions"]), observed
        results[language] = observed
    return results


def main():
    parser = argparse.ArgumentParser()
    for name in ("workspace", "tools", "evidence", "dependencies"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--scenario", choices=("cross", "migration", "contracts"), required=True)
    options = parser.parse_args()
    for name in ("workspace", "tools", "evidence", "dependencies"):
        setattr(options, name, getattr(options, name).resolve())
    options.evidence.mkdir(parents=True)
    result = {"cross": cross, "migration": migration, "contracts": contracts}[options.scenario](options)
    save(options.evidence / "result.json", result)


if __name__ == "__main__":
    main()
