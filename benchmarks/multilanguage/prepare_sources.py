import argparse
from pathlib import Path
import shutil


parser = argparse.ArgumentParser()
parser.add_argument("--workspace", type=Path, required=True)
parser.add_argument("--destination", type=Path, required=True)
options = parser.parse_args()
root = options.workspace.resolve()
destination = options.destination.resolve()
destination.mkdir(parents=True)
python = destination / "python"
shutil.copytree(root / "plugins/kv-py", python, ignore=shutil.ignore_patterns("__pycache__"))
shutil.copytree(root / "sdks/python/cordis_sdk", python / "cordis_sdk", ignore=shutil.ignore_patterns("__pycache__"))
typescript = destination / "typescript"
shutil.copytree(root / "plugins/kv-ts", typescript, ignore=shutil.ignore_patterns("__pycache__", "typescript", "@types", "undici-types", "src", "tsconfig.json"))
sdk = typescript / "node_modules/cordis-sdk/dist"
shutil.rmtree(sdk)
shutil.copytree(root / "sdks/typescript/cordis-sdk/dist", sdk)
