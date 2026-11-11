from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

DESCRIPTION = """一致性矩阵工具（仅依赖 Python 标准库）。

对象：
- 上游用例清单：cordis/packages/core/tests/*.spec.ts 中的 it(...) / it.each(...)(...) 声明。
- 一致性矩阵：docs/04-conformance.md 中以 `| <ID> |` 开头的表格行。

用法（在工作区根目录执行）：
  python tools/conformance.py list                 # 打印上游用例清单
  python tools/conformance.py init                 # 输出矩阵表格（初始状态），用于首次生成
  python tools/conformance.py check                # 校验矩阵覆盖全部上游用例且字段合法
  python tools/conformance.py check --through M3   # 另要求 M0..M3 的用例均已离开“未开始”
"""

ROOT = Path(__file__).resolve().parent.parent
TESTS = ROOT / "cordis" / "packages" / "core" / "tests"
MATRIX = ROOT / "docs" / "04-conformance.md"

# spec 文件 -> (ID 前缀, 默认里程碑)。顺序即矩阵分节顺序。
SPECS = {
    "dispose.spec.ts": ("DSP", "M1"),
    "events.spec.ts": ("EVT", "M2"),
    "component.spec.ts": ("CMP", "M3"),
    "fiber.spec.ts": ("FBR", "M3"),
    "update.spec.ts": ("UPD", "M3"),
    "service.spec.ts": ("SVC", "M4"),
    "service-dispose.spec.ts": ("SVD", "M4"),
    "reflect.spec.ts": ("RFL", "M4"),
    "isolate.spec.ts": ("ISO", "M4"),
    "intercept.spec.ts": ("ICP", "M5"),
    "shadow.spec.ts": ("SHD", "M5"),
    "associate.spec.ts": ("ASC", "M5"),
    "invoke.spec.ts": ("INV", "M5"),
    "logger.spec.ts": ("LOG", "M5"),
    "replace.spec.ts": ("RPL", "M5"),
    "decorator.spec.ts": ("DEC", "M6"),
}

# 个别用例依赖更晚里程碑的能力，按 (文件, 用例名) 覆盖默认里程碑。
OVERRIDES = {
    # 该用例通过 ctx.use 实例化组件，依赖 M3 的组件生命周期。
    ("dispose.spec.ts", "dispose by component"): "M3",
    ("events.spec.ts", "skips a listener disposed by an earlier component during dispatch"): "M3",
    ("events.spec.ts", "internal/update continues past a removed fiber listener"): "M3",
    ("events.spec.ts", "does not accumulate across reloads"): "M3",
    ("events.spec.ts", "can be disposed explicitly"): "M3",
    ("events.spec.ts", "runs prepended listeners first"): "M3",
    ("events.spec.ts", "global listeners are not tied to a child fiber reload"): "M3",
    # 以下用例的场景使用 provide / inject / Service，依赖 M4 的余效应能力。
    ("fiber.spec.ts", "inertia lock 1"): "M4",
    ("fiber.spec.ts", "inertia lock 2"): "M4",
    ("fiber.spec.ts", "inertia lock 3"): "M4",
    ("fiber.spec.ts", "failed fiber does not re-enter on dependency refresh"): "M4",
    ("fiber.spec.ts", "update recovers a failed fiber"): "M4",
    ("fiber.spec.ts", "settles a pending fiber to DISPOSED on dispose"): "M4",
    ("fiber.spec.ts", "update config while injected service reloads"): "M4",
    ("update.spec.ts", "settles concurrent updates on the latest config (inject: %s)"): "M4",
    ("update.spec.ts", "waits for dependencies and loads only the latest config"): "M4",
    ("component.spec.ts", "Service.init"): "M4",
}

STATUSES = ("未开始", "已移植", "已改写", "不适用")
MILESTONES = ("M0", "M1", "M2", "M3", "M4", "M5", "M6")

IT_RE = re.compile(r"^\s*it(\.each\(.*\))?\(")
STR_RE = re.compile(r"""(['"`])((?:\\.|(?!\1).)*)\1""")


def milestone_for(spec: str, title: str) -> str:
    if (spec, title) in OVERRIDES:
        return OVERRIDES[(spec, title)]
    if spec == "dispose.spec.ts" and "async" in title:
        return "M3"
    return SPECS[spec][1]


def upstream_cases():
    """返回 [(id, spec, line, title, milestone)]，顺序为 SPECS 顺序 + 文件内出现顺序。"""
    cases = []
    found = {p.name for p in TESTS.glob("*.spec.ts")}
    unknown = found - set(SPECS)
    missing = set(SPECS) - found
    if unknown or missing:
        sys.exit(f"spec 文件集合与工具内置清单不一致：多出 {sorted(unknown)}，缺少 {sorted(missing)}")
    for spec, (prefix, _) in SPECS.items():
        lines = (TESTS / spec).read_text(encoding="utf-8").splitlines()
        n = 0
        for i, line in enumerate(lines):
            m = IT_RE.match(line)
            if not m:
                continue
            # it.each 的用例名位于 `)(` 之后；若该行没有字符串字面量，则取下一行。
            rest = line[m.end():]
            s = STR_RE.search(rest)
            if not s and i + 1 < len(lines):
                s = STR_RE.search(lines[i + 1])
            if not s:
                sys.exit(f"无法解析用例名：{spec}:{i + 1}")
            title = s.group(2).replace("\\'", "'").replace("|", "\\|")
            n += 1
            cases.append((f"{prefix}-{n:02d}", spec, i + 1, title, milestone_for(spec, title)))
    return cases


def cmd_list(_args):
    for cid, spec, line, title, ms in upstream_cases():
        print(f"{cid}\t{ms}\t{spec}:{line}\t{title}")


def cmd_init(_args):
    cases = upstream_cases()
    for spec, (prefix, _) in SPECS.items():
        rows = [c for c in cases if c[1] == spec]
        print(f"\n### {prefix}：`{spec}`（{len(rows)} 条）\n")
        print("| ID | 上游位置 | 上游用例 | 里程碑 | 状态 | Rust 测试 / 理由 |")
        print("|---|---|---|---|---|---|")
        for cid, _spec, line, title, ms in rows:
            print(f"| {cid} | `{spec}:{line}` | {title} | {ms} | 未开始 | |")


def parse_matrix():
    rows = {}
    row_re = re.compile(r"^\|\s*([A-Z]{3}-\d{2})\s*\|")
    for line in MATRIX.read_text(encoding="utf-8").splitlines():
        if not row_re.match(line):
            continue
        cells = [c.strip() for c in re.split(r"(?<!\\)\|", line)[1:-1]]
        if len(cells) != 6:
            sys.exit(f"矩阵行列数应为 6：{line}")
        rows[cells[0]] = cells
    return rows


def cmd_check(args):
    cases = upstream_cases()
    rows = parse_matrix()
    errors = []
    counts = {s: 0 for s in STATUSES}
    ids = {c[0] for c in cases}
    for extra in sorted(set(rows) - ids):
        errors.append(f"{extra}: 矩阵中存在上游没有的用例 ID")
    through = MILESTONES.index(args.through) if args.through else -1
    for cid, spec, line, title, ms in cases:
        row = rows.get(cid)
        if row is None:
            errors.append(f"{cid}: 矩阵缺少该用例（{spec}:{line} {title}）")
            continue
        _, loc, row_title, row_ms, status, note = row
        if loc.strip("`") != f"{spec}:{line}":
            errors.append(f"{cid}: 上游位置应为 {spec}:{line}，矩阵为 {loc}")
        if row_title != title:
            errors.append(f"{cid}: 用例名与上游不一致")
        if row_ms not in MILESTONES:
            errors.append(f"{cid}: 里程碑非法：{row_ms}")
        if status not in STATUSES:
            errors.append(f"{cid}: 状态非法：{status}")
            continue
        counts[status] += 1
        if status in ("已移植", "已改写") and "::" not in note and ".rs" not in note:
            errors.append(f"{cid}: 状态为{status}，但未填写 Rust 测试位置")
        if status in ("已改写", "不适用") and len(note) < 8:
            errors.append(f"{cid}: 状态为{status}，但未填写理由")
        if through >= 0 and row_ms in MILESTONES and MILESTONES.index(row_ms) <= through and status == "未开始":
            errors.append(f"{cid}: 属于 {row_ms}，在 --through {args.through} 下不得为未开始")
    total = len(cases)
    print(f"上游用例 {total} 条；" + "，".join(f"{s} {counts[s]}" for s in STATUSES))
    if errors:
        print(f"\n发现 {len(errors)} 个问题：")
        for e in errors:
            print("  - " + e)
        sys.exit(1)
    print("矩阵校验通过。")


def main():
    parser = argparse.ArgumentParser(description=DESCRIPTION, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list").set_defaults(fn=cmd_list)
    sub.add_parser("init").set_defaults(fn=cmd_init)
    check = sub.add_parser("check")
    check.add_argument("--through", choices=MILESTONES)
    check.set_defaults(fn=cmd_check)
    args = parser.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
