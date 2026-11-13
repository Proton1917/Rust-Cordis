import json
import pathlib
import subprocess


root = pathlib.Path(__file__).resolve().parents[1]
scratch = root / "target" / "tmp"
scratch.mkdir(parents=True, exist_ok=True)
sources = sorted(
    path.relative_to(root)
    for category in ("crates", "plugins")
    for path in root.glob(f"{category}/*/src")
)
sources += [pathlib.Path("sdks/python/cordis_sdk")]
sources += sorted(path.relative_to(root) for path in root.glob("sdks/typescript/*/src"))

# DOC-13 §1.1：使用 cloc 的 Rust 注释规则，逐文件计数并关闭内容去重。
result = subprocess.run(
    [
        "cloc",
        "--skip-uniqueness",
        f"--sdir={scratch}",
        "--include-lang=Rust,Python,TypeScript",
        "--json",
        "--by-file",
        *(str(path) for path in sources),
    ],
    cwd=root,
    capture_output=True,
    text=True,
    check=True,
)
records = json.loads(result.stdout)
totals = {}
for filename, counts in records.items():
    parts = pathlib.Path(filename).parts
    if len(parts) < 3 or parts[0] not in ("crates", "plugins", "sdks"):
        continue
    if parts[0] != "sdks" and counts["language"] != "Rust":
        continue
    name = "/".join(parts[:3]) if parts[0] == "sdks" else parts[1]
    total = totals.setdefault(name, [0, 0])
    total[0] += counts["code"]
    total[1] += counts["code"] + counts["comment"] + counts["blank"]

print(f"cloc {records['header']['cloc_version']}")
print("| crate | 代码行 / 总行 |")
print("|---|---:|")
for name, (code, total) in sorted(totals.items()):
    print(f"| {name} | {code} / {total} |")
core_names = (
    "cordis", "cordis-supervisor", "cordis-runtime",
    "cordis-up", "cordis-process", "cordis-types", "cordis-routing", "cordis-channels",
    "cordis-web-deployment",
)
core_code = sum(totals[name][0] for name in core_names)
core_total = sum(totals[name][1] for name in core_names)
print(f"| 核心进程 crate 合计 | {core_code} / {core_total} |")
