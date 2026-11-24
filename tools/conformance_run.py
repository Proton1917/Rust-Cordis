"""Execute the Rust evidence named by every applicable upstream conformance row.

This checks the scenario matrix against actual compiler and test-harness output.
It does not establish equivalence outside the executions of the selected tests.
The read-only upstream checkout must be at the documented immutable revision.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path
from types import SimpleNamespace

import conformance

ROOT = Path(__file__).resolve().parent.parent
UPSTREAM_REVISION = "261c5fdae6edb54e7ed4f071024f530acd18903b"
LANGUAGE_SPECIFIC = {"EVT-05", "RFL-01", "ASC-06"}
REFERENCE = re.compile(r"`(crates/cordis/(tests|src)/[^`\s]+\.rs)::([^`\s]+)`")
OUTCOME = re.compile(r"^test (.+?) \.\.\. (ok|FAILED|ignored)(?:\b.*)?$", re.MULTILINE)


class VerificationError(Exception):
    pass


def command(argv, *, cwd=ROOT, timeout=180):
    started = time.monotonic()
    try:
        result = subprocess.run(
            [str(arg) for arg in argv], cwd=cwd, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout,
            env={**os.environ, "CARGO_TERM_COLOR": "never"},
        )
    except subprocess.TimeoutExpired as error:
        raise VerificationError(f"command timed out after {timeout}s: {argv}") from error
    except OSError as error:
        raise VerificationError(f"cannot run {argv[0]}: {error}") from error
    record = {"command": [str(arg) for arg in argv], "exit_code": result.returncode,
              "elapsed_seconds": round(time.monotonic() - started, 3),
              "stdout": result.stdout, "stderr": result.stderr}
    if result.returncode:
        raise VerificationError(
            f"command exited {result.returncode}: {' '.join(map(str, argv))}\n"
            f"{result.stdout}\n{result.stderr}"
        )
    return record


def references(rows):
    selected, excluded = {}, {}
    for case_id, row in rows.items():
        status, note = row[4:6]
        if status == "不适用":
            excluded[case_id] = {"status": "language_specific", "reason": note}
            continue
        if status not in ("已移植", "已改写"):
            raise VerificationError(f"{case_id}: scenario is not implemented ({status})")
        matches = REFERENCE.findall(note)
        if len(matches) != 1:
            raise VerificationError(f"{case_id}: expected exactly one executable Rust reference")
        path, kind, name = matches[0]
        if not (ROOT / path).is_file():
            raise VerificationError(f"{case_id}: source file is missing: {path}")
        selected[case_id] = {"path": path, "kind": kind, "name": name}
    if not selected:
        raise VerificationError("zero selected Rust scenarios")
    if excluded.keys() != LANGUAGE_SPECIFIC:
        raise VerificationError("language-specific exclusions differ from the reviewed matrix baseline")
    return selected, excluded


def compiled_targets(stdout, expected):
    artifacts = {}
    for line in stdout.splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if item.get("reason") != "compiler-artifact" or not item.get("executable"):
            continue
        target = item["target"]
        source = Path(target["src_path"]).resolve()
        for path in expected:
            if source == (ROOT / path).resolve() and "test" in target["kind"]:
                artifacts[path] = item["executable"]
    missing = sorted(set(expected) - artifacts.keys())
    if missing:
        raise VerificationError(f"Cargo produced no test executable for: {missing}")
    return artifacts


def test_names(stdout):
    return {line[:-6] for line in stdout.splitlines() if line.endswith(": test")}


def passed_tests(stdout):
    results = {}
    for name, status in OUTCOME.findall(stdout):
        if name in results:
            raise VerificationError(f"duplicate test outcome: {name}")
        results[name] = status
    return results


def require_passed(name, results, case_id):
    if results.get(name) != "ok":
        raise VerificationError(f"{case_id}: {name} outcome is {results.get(name, 'not executed')}")


def matching_doctests(reference, results):
    path, name = reference["path"], reference["name"]
    # rustdoc reports the module prefix, the item and its current source line.
    pattern = re.compile(
        rf"^{re.escape(path)} - (?:[A-Za-z0-9_]+::)*{re.escape(name)} "
        r"\(line \d+\) - compile fail$"
    )
    return sorted(test for test in results if pattern.fullmatch(test))


def source_digest():
    # Bind evidence to source bytes even while changes are still uncommitted.
    paths = set((ROOT / "crates/cordis").rglob("*.rs"))
    paths.update((ROOT / "crates/cordis-testkit").rglob("*.rs"))
    paths.update(ROOT / name for name in (
        "Cargo.toml", "Cargo.lock", "crates/cordis/Cargo.toml",
        "crates/cordis-testkit/Cargo.toml", "docs/04-conformance.md",
        "tools/conformance.py", "tools/conformance_run.py",
    ))
    return {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(paths)}


def verify(args, report):
    upstream = args.upstream.resolve()
    head = command(["git", "rev-parse", "HEAD"], cwd=upstream)["stdout"].strip()
    if head != UPSTREAM_REVISION:
        raise VerificationError(f"upstream revision must be {UPSTREAM_REVISION}; found {head}")
    dirty = command(["git", "status", "--porcelain", "--untracked-files=all",
                     "--", "packages/core/tests"], cwd=upstream)["stdout"]
    if dirty:
        raise VerificationError(f"upstream test files differ from the pinned revision:\n{dirty}")
    conformance.TESTS = upstream / "packages/core/tests"
    conformance.cmd_check(SimpleNamespace(through="M6"))
    selected, excluded = references(conformance.parse_matrix())
    report.update({"upstream_revision": head, "source_sha256": source_digest(),
                   "rust_revision": command(["git", "rev-parse", "HEAD"])["stdout"].strip(),
                   "cargo_version": command([args.cargo, "--version"])["stdout"].strip(),
                   "rustc_version": command(["rustc", "--version"])["stdout"].strip(),
                   "cases": dict(excluded), "commands": [], "status": "running"})
    test_paths = sorted({ref["path"] for ref in selected.values() if ref["kind"] == "tests"})
    build = [args.cargo, "test", "--locked", "-p", "cordis", "--no-run", "--message-format=json"]
    for path in test_paths:
        build.extend(["--test", Path(path).stem])
    print(f"Compiling {len(test_paths)} Rust test targets for {len(selected)} applicable rows.", flush=True)
    built = command(build, timeout=args.build_timeout)
    report["commands"].append(built)
    binaries = compiled_targets(built["stdout"], test_paths)
    for path, binary in sorted(binaries.items()):
        listed = command([binary, "--list", "--format", "terse"], timeout=args.test_timeout)
        names = test_names(listed["stdout"])
        relevant = {case_id: ref for case_id, ref in selected.items() if ref["path"] == path}
        for case_id, ref in relevant.items():
            if ref["name"] not in names:
                raise VerificationError(f"{case_id}: {ref['name']} is absent from the compiled test inventory")
        executed = command([binary, "--test-threads=1", "--color", "never"], timeout=args.test_timeout)
        report["commands"].extend([listed, executed])
        outcomes = passed_tests(executed["stdout"])
        for case_id, ref in relevant.items():
            require_passed(ref["name"], outcomes, case_id)
            report["cases"][case_id] = {**ref, "status": "passed", "test": ref["name"]}
        print(f"{Path(path).stem}: {len(relevant)} conformance rows executed.", flush=True)
    doctest_rows = {case_id: ref for case_id, ref in selected.items() if ref["kind"] == "src"}
    if doctest_rows:
        executed = command([args.cargo, "test", "--locked", "-p", "cordis", "--doc", "--",
                            "--color", "never"], timeout=args.build_timeout)
        report["commands"].append(executed)
        outcomes = passed_tests(executed["stdout"])
        for case_id, ref in doctest_rows.items():
            matches = matching_doctests(ref, outcomes)
            if not matches:
                raise VerificationError(f"{case_id}: no executed compile-fail doctest for {ref}")
            for test in matches:
                require_passed(test, outcomes, case_id)
            report["cases"][case_id] = {**ref, "status": "passed", "tests": matches}
    if source_digest() != report["source_sha256"]:
        raise VerificationError("source changed during verification; rerun against a stable tree")
    if len(report["cases"]) != len(selected) + len(excluded):
        raise VerificationError("incomplete conformance result set")
    report.update({"status": "passed", "applicable_rows": len(selected),
                   "language_specific_rows": len(excluded),
                   "unique_rust_references": len({(r['path'], r['name']) for r in selected.values()})})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", type=Path, default=ROOT / "cordis")
    parser.add_argument("--output", type=Path, default=ROOT / "target/conformance-report.json")
    parser.add_argument("--cargo", default=os.environ.get("CARGO", "cargo"))
    parser.add_argument("--build-timeout", type=float, default=600)
    parser.add_argument("--test-timeout", type=float, default=180)
    args = parser.parse_args()
    report = {"schema": "cordis-conformance-execution/v1", "status": "failed",
              "scope": "Executed upstream scenario adaptations; not a formal equivalence proof"}
    started = time.monotonic()
    try:
        verify(args, report)
    except (VerificationError, SystemExit) as error:
        report.update({"status": "failed", "error": str(error)})
    finally:
        report["elapsed_seconds"] = round(time.monotonic() - started, 3)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if report["status"] != "passed":
        print(report.get("error", "verification failed"), file=sys.stderr)
        return 1
    print(f"Verified {report['applicable_rows']} applicable rows; "
          f"{report['language_specific_rows']} documented language-specific exclusions. "
          f"Evidence: {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
