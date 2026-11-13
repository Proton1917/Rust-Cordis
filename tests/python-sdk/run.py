import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess


def command(arguments, log, cwd, environment):
    """DOC-15 M18：保留实际命令的输出与退出码，失败终止当前场景。"""
    with log.open("w") as output:
        subprocess.run(
            arguments,
            cwd=cwd,
            env=environment,
            stdout=output,
            stderr=subprocess.STDOUT,
            check=True,
        )


def prepare(options):
    """DOC-15 M18：接收公共 testkit 的构建快照，形成独立运行与证据目录。"""
    evidence = options.evidence.resolve()
    source = options.source.resolve()
    tools = options.tools.resolve()
    evidence.mkdir(parents=True)
    binaries = evidence / "bin"
    binaries.mkdir()
    names = [
        "cordis",
        "cordis-dev",
        "cordis-host",
        "cordis-runtime-fixture",
        "libkv_mem.dylib",
        "libcounter.dylib",
        "libkv_contract.dylib",
    ]
    for name in names:
        shutil.copy2(tools / name, binaries / name)
    dependencies = (
        options.dependencies.resolve()
        if options.dependencies
        else evidence / "dependencies"
    )
    environment = os.environ.copy()
    environment.update(
        {
            "CORDIS_TEST_WORKSPACE": str(source),
            "CORDIS_TEST_SOURCE": str(source),
            "CORDIS_TEST_EVIDENCE": str(evidence),
            "CORDIS_TEST_DEPENDENCIES": str(dependencies),
            "PIP_REQUIRE_VIRTUALENV": "false",
            "TMPDIR": str(evidence / "tmp"),
        }
    )
    (evidence / "tmp").mkdir()
    python = ["conda", "run", "--no-capture-output", "-n", "base", "python"]
    if options.dependencies is None:
        command(
            python
            + [
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-deps",
                "--cache-dir",
                str(source / "target/python-sdk-dependencies/pip-cache"),
                "--target",
                str(dependencies),
                "-r",
                str(source / "sdks/python/requirements.txt"),
            ],
            evidence / "dependencies.log",
            source,
            environment,
        )
    bindings = evidence / "bindings"
    bindings.mkdir()
    command(
        [
            str(binaries / "cordis-dev"),
            "idl",
            "gen",
            str(Path(__file__).with_name("resources.wit")),
            "--lang",
            "python",
            "--output",
            str(bindings / "resources_binding.py"),
        ],
        evidence / "resources-bindings.log",
        source,
        environment,
    )
    (evidence / "inputs.json").write_text(
        json.dumps(
            {
                "source": str(source),
                "tools": str(tools),
                "files": names,
                "dependencies": str(dependencies),
            },
            indent=2,
        )
        + "\n"
    )
    return evidence, environment, python


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--source", type=Path, default=Path(__file__).resolve().parents[2]
    )
    parser.add_argument("--tools", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--dependencies", type=Path)
    parser.add_argument(
        "--scenario",
        choices=("smoke", "contract", "resources", "transfer", "realms", "migration"),
        action="append",
    )
    options = parser.parse_args()
    evidence, environment, python = prepare(options)
    scenarios = options.scenario or [
        "smoke",
        "contract",
        "resources",
        "transfer",
        "realms",
        "migration",
    ]
    results = []
    for scenario in scenarios:
        script = "resources" if scenario == "transfer" else scenario
        arguments = python + [str(Path(__file__).with_name(script + ".py"))]
        if scenario == "transfer":
            arguments.append("transfer")
        command(
            arguments,
            evidence / (scenario + ".log"),
            options.source.resolve(),
            environment,
        )
        results.append({"scenario": scenario, "exit": 0})
        print(f"通过 {scenario}", flush=True)
    (evidence / "result.json").write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
