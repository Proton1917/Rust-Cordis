import json
import pathlib
import subprocess


root = pathlib.Path(__file__).resolve().parents[1]
scratch = root / "target" / "tmp"
scratch.mkdir(parents=True, exist_ok=True)
sources = sorted(path.relative_to(root) for path in root.glob("crates/*/src"))

# DOC-13 §1.1：使用 cloc 的 Rust 注释规则，逐文件计数并关闭内容去重。
result = subprocess.run(
    [
        "cloc",
        "--skip-uniqueness",
        f"--sdir={scratch}",
        "--include-lang=Rust",
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
    if len(parts) < 3 or parts[0] != "crates":
        continue
    total = totals.setdefault(parts[1], [0, 0])
    total[0] += counts["code"]
    total[1] += counts["code"] + counts["comment"] + counts["blank"]

print(f"cloc {records['header']['cloc_version']}")
print("| crate | 代码行 / 总行 |")
print("|---|---:|")
for name, (code, total) in sorted(totals.items()):
    print(f"| {name} | {code} / {total} |")
