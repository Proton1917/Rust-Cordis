import json
import os
from pathlib import Path
import shlex
import subprocess
import sys


def build_environment(project, target=None):
    home = Path(os.environ.get("HOME") or os.environ["USERPROFILE"])
    sources = {
        "home": str(home),
        "cargo_home": str(Path(os.environ.get("CARGO_HOME", home / ".cargo")).absolute()),
        "sysroot": subprocess.check_output([os.environ.get("RUSTC", "rustc"), "--print", "sysroot"], text=True).strip(),
        "project": str(Path(project).resolve()),
    }
    rules = json.loads(Path(__file__).with_name("path-remapping.json").read_text())
    is_windows = "-windows-" in target if target else sys.platform == "win32"
    is_msvc = target.endswith("-msvc") if target else sys.platform == "win32"
    rust = [] if is_windows else list(rules["rust_flags"])
    native = []
    for source, destination in rules["prefixes"]:
        rust.append(f"--remap-path-prefix={sources[source]}={destination}")
        if not is_msvc:
            native.extend(flag.replace("{from}", sources[source]).replace("{to}", destination)
                          for flag in rules["native_flags"])
    apple = "-apple-" in target if target else sys.platform == "darwin"
    if apple:
        rust.extend(flag.replace("{home}", sources["home"]) for flag in rules["apple_link_flags"])
    if "CARGO_ENCODED_RUSTFLAGS" in os.environ:
        rust.extend(flag for flag in os.environ["CARGO_ENCODED_RUSTFLAGS"].split("\x1f") if flag)
    else:
        rust.extend(os.environ.get("RUSTFLAGS", "").split())
    if is_msvc:
        return {"CARGO_ENCODED_RUSTFLAGS": "\x1f".join(rust)}
    flags = {"CFLAGS": list(native), "CXXFLAGS": list(native)}
    shell = os.environ.get("CC_SHELL_ESCAPED_FLAGS", "") not in ("", "0", "false", "no")
    for name, value in os.environ.items():
        if any(name == base or name.startswith(base + "_") or name.endswith("_" + base)
               for base in ("CFLAGS", "CXXFLAGS", "ARFLAGS", "RANLIBFLAGS")):
            flags.setdefault(name, []).extend(shlex.split(value) if shell else value.split())
    result = {name: shlex.join(values) for name, values in flags.items()}
    result.update({"CC_SHELL_ESCAPED_FLAGS": "1", "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(rust)})
    return result


def encoded_flags(project):
    return build_environment(project)["CARGO_ENCODED_RUSTFLAGS"]


if __name__ == "__main__":
    print(encoded_flags(sys.argv[1] if len(sys.argv) > 1 else Path.cwd()))
