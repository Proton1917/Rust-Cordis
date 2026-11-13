import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess


ROOT = Path(__file__).resolve().parents[2]


def command(arguments, log, environment):
    with log.open("w") as output:
        subprocess.run(arguments, cwd=ROOT, env=environment, stdout=output, stderr=subprocess.STDOUT, check=True)


def prepare(options):
    """DOC-15 M19：公共 testkit 传入实际运行产物，npm 依赖与证据保持在工作区。"""
    evidence = options.evidence.resolve()
    evidence.mkdir(parents=True)
    (evidence / "tmp").mkdir()
    (evidence / "bin").mkdir()
    for source in options.tools.resolve().iterdir():
        if source.is_file():
            shutil.copy2(source, evidence / "bin" / source.name)
    environment = {**os.environ, "CORDIS_TEST_EVIDENCE": str(evidence), "TMPDIR": str(evidence / "tmp")}
    cli = str(evidence / "bin/cordis-dev")
    fixture = ROOT / "tests/typescript-sdk/plugin"
    for source, destination in [
        (ROOT / "plugins/kv-api/kv.wit", fixture / "src/bindings.ts"),
        (ROOT / "tests/typescript-sdk/resources.wit", fixture / "src/resources-binding.ts"),
    ]:
        command([cli, "idl", "gen", "--lang", "ts", "--output", str(destination), str(source)], evidence / f"generate-{destination.stem}.log", environment)
    for name, directory in [("sdk", ROOT / "sdks/typescript/cordis-sdk"), ("kv", ROOT / "plugins/kv-ts"), ("fixtures", fixture)]:
        command(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund", "--cache", str(ROOT / "target/npm-cache"), "--prefix", str(directory)], evidence / f"{name}-install.log", environment)
        command(["npm", "run", "build", "--prefix", str(directory)], evidence / f"{name}-build.log", environment)
    return evidence, environment


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--tools", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    options = parser.parse_args()
    evidence, environment = prepare(options)
    scenarios = [
        ("typescript-provider", "acceptance.py", ["typescript-provider"]),
        ("typescript-consumer", "acceptance.py", ["typescript-consumer"]),
        ("contract", "acceptance.py", ["contract"]),
        ("resources", "resources.py", ["resources"]),
        ("realms", "realms.py", []),
        ("migration", "migration.py", ["migration"]),
        ("rollback", "migration.py", ["rollback", "rollback"]),
    ]
    result = []
    for name, script, arguments in scenarios:
        command(["conda", "run", "--no-capture-output", "-n", "base", "python", str(Path(__file__).with_name(script)), *arguments], evidence / f"{name}.log", environment)
        result.append({"scenario": name, "exit": 0})
        print(f"通过 {name}", flush=True)
    (evidence / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
