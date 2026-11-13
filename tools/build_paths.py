import json
import os
from pathlib import Path
import subprocess
import sys


def encoded_flags(project):
    """DOC-15 §12.2：规则顺序来自共享表，现有 Cargo 环境参数保留后置优先。"""
    home = Path(os.environ["HOME"])
    sources = {
        "home": home,
        "cargo_home": Path(os.environ.get("CARGO_HOME", home / ".cargo")).absolute(),
        "sysroot": Path(subprocess.check_output([os.environ.get("RUSTC", "rustc"), "--print", "sysroot"], text=True).strip()),
        "project": Path(project).resolve(),
    }
    definitions = json.loads(Path(__file__).with_name("path-remapping.json").read_text())
    flags = [destination if source == "flag" else f"--remap-path-prefix={sources[source]}={destination}" for source, destination in definitions]
    if "CARGO_ENCODED_RUSTFLAGS" in os.environ:
        flags.extend(flag for flag in os.environ["CARGO_ENCODED_RUSTFLAGS"].split("\x1f") if flag)
    else:
        flags.extend(os.environ.get("RUSTFLAGS", "").split())
    return "\x1f".join(flags)


if __name__ == "__main__":
    print(encoded_flags(sys.argv[1] if len(sys.argv) > 1 else Path.cwd()))
