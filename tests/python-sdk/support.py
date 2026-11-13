import json
import os
from pathlib import Path


ROOT = Path(
    os.environ.get("CORDIS_TEST_WORKSPACE", Path(__file__).resolve().parents[2])
).resolve()
SOURCE = Path(os.environ.get("CORDIS_TEST_SOURCE", ROOT)).resolve()
EVIDENCE = Path(
    os.environ.get("CORDIS_TEST_EVIDENCE", ROOT / "target/python-sdk-evidence")
).resolve()
DEPENDENCIES = Path(
    os.environ.get("CORDIS_TEST_DEPENDENCIES", EVIDENCE / "dependencies")
).resolve()


def test_environment():
    """DOC-15 M18：真实子进程使用验收副本与明确的 Python 依赖路径。"""
    environment = os.environ.copy()
    environment["PYTHONPATH"] = os.pathsep.join(
        map(
            str,
            [
                SOURCE / "sdks/python",
                SOURCE / "plugins/kv-py",
                SOURCE / "plugins/counter-py",
                DEPENDENCIES,
                EVIDENCE / "bindings",
            ],
        )
    )
    environment["PATH"] = str(EVIDENCE / "bin") + os.pathsep + environment["PATH"]
    return environment


def manifest(path, name, command):
    path.mkdir(parents=True, exist_ok=True)
    (path / "cordis-plugin.toml").write_text(
        f'name={json.dumps(name)}\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n'
    )
