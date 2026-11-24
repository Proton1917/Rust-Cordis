#!/usr/bin/env python3
"""Check Lean proofs, generated production decisions, and the compiled finite bridge.

Requires Lean/Lake 4.19.0 and the repository's Rust toolchain. No Python packages.
The returned evidence separates machine proofs, finite exhaustive execution, and
the integration tests that exercise the real scheduler and external callbacks.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
FORMAL = ROOT / "formal"
GENERATED = ROOT / "crates/cordis/src/lifecycle_generated.rs"
PROOFS = {"Lifecycle.lean": "Cordis", "Effects.lean": "Cordis.Effects", "Admission.lean": "Cordis.Admission"}
ARITIES = {"idle": 3, "target": 2, "refresh": 7, "finish": 5, "settled": 4, "current": 2, "current_epoch": 3, "cleanup": 2}
ALLOWED_AXIOMS = {"propext", "Quot.sound", "Classical.choice"}


def run(command: list[str], *, cwd: Path = FORMAL, success: bool = True,
        input_text: str | None = None) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=180, input=input_text)
    if success and result.returncode:
        raise RuntimeError(f"command failed: {' '.join(command)}\n{result.stdout}{result.stderr}")
    return result


def lean_code(text: str) -> str:
    """Remove nested Lean comments before checking forbidden proof escapes."""
    result, depth, i = [], 0, 0
    while i < len(text):
        if text.startswith("/-", i):
            depth += 1
            i += 2
        elif depth and text.startswith("-/", i):
            depth -= 1
            i += 2
        elif depth:
            i += 1
        elif text.startswith("--", i):
            end = text.find("\n", i)
            i = len(text) if end < 0 else end
        else:
            result.append(text[i])
            i += 1
    if depth:
        raise RuntimeError("unterminated Lean comment")
    return "".join(result)


def theorems() -> list[str]:
    names = []
    for filename, namespace in PROOFS.items():
        code = lean_code((FORMAL / filename).read_text())
        forbidden = re.search(r"\b(sorry|admit|axiom|native_decide|unsafe|implemented_by|skipKernelTC)\b", code)
        if forbidden:
            raise RuntimeError(f"proof escape in {filename}: {forbidden.group(0)}")
        names += [f"{namespace}.{name}" for name in re.findall(r"^theorem\s+(\w+)", code, re.M)]
    if not names or len(names) != len(set(names)):
        raise RuntimeError("proof declarations must be nonempty and unique")
    return names


def parse_vectors(text: str) -> dict[tuple[str, int], str]:
    values = {}
    for line in text.splitlines():
        fields = line.split("\t")
        if len(fields) != 3:
            raise RuntimeError(f"invalid decision output: {line!r}")
        name, raw_mask, value = fields
        mask = int(raw_mask)
        if name not in ARITIES or not 0 <= mask < 2 ** ARITIES[name] or (name, mask) in values:
            raise RuntimeError(f"unexpected or repeated decision case: {line!r}")
        values[name, mask] = value
    expected_count = sum(2 ** n for n in ARITIES.values())
    if len(values) != expected_count:
        raise RuntimeError(f"incomplete decision input domain: {len(values)}/{expected_count}")
    return values


def probe(rustc: str, root: Path, output: Path) -> str:
    run([rustc, "--edition=2024", "-O", str(root / "formal/decision_probe.rs"), "-o", str(output)])
    return run([str(output)]).stdout


def source_paths() -> list[Path]:
    return [FORMAL / name for name in PROOFS] + [
        FORMAL / "Generate.lean", FORMAL / "decision_probe.rs", FORMAL / "lakefile.toml",
        FORMAL / "lake-manifest.json", FORMAL / "lean-toolchain", GENERATED,
        ROOT / "rust-toolchain.toml", ROOT / "crates/cordis/src/lifecycle.rs",
        ROOT / "crates/cordis/src/fiber.rs", ROOT / "crates/cordis/src/effect.rs",
        ROOT / "crates/cordis/src/context.rs", ROOT / "crates/cordis/src/registry.rs",
        ROOT / "crates/cordis/src/lib.rs", Path(__file__).resolve(),
    ]


def snapshot(paths: list[Path]) -> dict[str, str]:
    return {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}


def require_same_source(expected: dict[str, str], observed: dict[str, str]) -> None:
    changed = sorted(key for key in expected.keys() | observed.keys() if expected.get(key) != observed.get(key))
    if changed:
        raise RuntimeError(f"verification inputs changed while checks were running: {changed}")


def write_evidence(path: Path | None, evidence: dict) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        json.dump(evidence, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def verify(args: argparse.Namespace, evidence: dict) -> dict:
    evidence["current_stage"] = "toolchain"
    lake = shutil.which(args.lake)
    rustc = shutil.which(args.rustc)
    rustfmt = shutil.which(args.rustfmt)
    if not rustfmt and rustc:
        sibling = Path(rustc).with_name("rustfmt")
        if sibling.is_file():
            rustfmt = str(sibling)
    if not lake or not rustc or not rustfmt:
        raise RuntimeError("Lean/Lake 4.19.0, rustc and rustfmt must be installed; no checks were skipped")
    lean_version = run([lake, "env", "lean", "--version"]).stdout.strip()
    evidence["lean"] = lean_version
    if "version 4.19.0," not in lean_version:
        raise RuntimeError(f"unexpected Lean version: {lean_version}")
    evidence["rustc"] = run([rustc, "--version"]).stdout.strip()
    evidence["rustfmt"] = run([rustfmt, "--version"]).stdout.strip()
    evidence["completed_stages"].append("toolchain")

    evidence["current_stage"] = "proof_build"
    tracked = source_paths()
    before = snapshot(tracked)
    evidence["source_before_sha256"] = dict(before)
    names = theorems()
    # A fresh proof build prevents stale .olean files from serving as evidence.
    run([lake, "clean"])
    build = run([lake, "build"])
    if re.search(r"\bsorry\b", build.stdout + build.stderr):
        raise RuntimeError("proof build contains an unresolved proof")
    evidence["completed_stages"].append("proof_build")
    with tempfile.TemporaryDirectory(prefix="cordis-semantics-") as raw_tmp:
        evidence["current_stage"] = "axiom_audit"
        temp = Path(raw_tmp)
        audit = temp / "Audit.lean"
        imports = "".join(f"import {Path(filename).stem}\n" for filename in PROOFS)
        audit.write_text(imports + "\n".join(f"#print axioms {name}" for name in names) + "\n")
        axioms = run([lake, "env", "lean", "--trust=0", str(audit)]).stdout
        seen = set()
        for name, dependencies in re.findall(r"'([^']+)' depends on axioms:\s*\[([^\]]*)\]", axioms):
            seen.add(name)
            actual = {item.strip() for item in dependencies.split(",") if item.strip()}
            if not actual <= ALLOWED_AXIOMS:
                raise RuntimeError(f"unapproved axiom dependency for {name}: {actual - ALLOWED_AXIOMS}")
        seen.update(re.findall(r"'([^']+)' does not depend on any axioms", axioms))
        if seen != set(names):
            raise RuntimeError(f"axiom audit incomplete: missing {set(names) - seen}, extra {seen - set(names)}")
        evidence["completed_stages"].append("axiom_audit")

        evidence["current_stage"] = "generation"
        generated = run([lake, "env", "lean", "--run", "Generate.lean", "rust"]).stdout
        generated = run([rustfmt, "--edition=2024", "--emit=stdout"], input_text=generated).stdout
        if args.write_generated:
            GENERATED.write_text(generated)
            before[str(GENERATED.relative_to(ROOT))] = hashlib.sha256(generated.encode()).hexdigest()
        elif GENERATED.read_text() != generated:
            raise RuntimeError("production lifecycle_generated.rs differs from the proven decision program")
        evidence["completed_stages"].append("generation")
        evidence["current_stage"] = "compiled_bridge"
        expected = parse_vectors(run([lake, "env", "lean", "--run", "Generate.lean", "vectors"]).stdout)
        observed = parse_vectors(probe(rustc, ROOT, temp / "decisions"))
        if observed != expected:
            mismatches = [key for key in expected if expected[key] != observed[key]]
            raise RuntimeError(f"compiled production decisions disagree: {mismatches}")
        evidence["completed_stages"].append("compiled_bridge")

        evidence["current_stage"] = "negative_controls"
        # An intentionally false theorem must be rejected by the proof checker.
        negative = temp / "Negative.lean"
        negative.write_text("import Lifecycle\nexample : Cordis.refresh false false false true true true true = Cordis.Action.unload := by decide\n")
        rejected = run([lake, "env", "lean", "--trust=0", str(negative)], success=False)
        if rejected.returncode == 0 or "error:" not in rejected.stdout + rejected.stderr:
            raise RuntimeError("proof negative control did not reject a false transition")

        # Mutate the actual emitted branch; the compiled bridge must detect it.
        copy = temp / "mutated"
        (copy / "formal").mkdir(parents=True)
        rust_dir = copy / "crates/cordis/src"
        rust_dir.mkdir(parents=True)
        shutil.copy2(FORMAL / "decision_probe.rs", copy / "formal/decision_probe.rs")
        shutil.copy2(ROOT / "crates/cordis/src/lifecycle.rs", rust_dir / "lifecycle.rs")
        corrupted = generated.replace("Action::Load", "Action::Unload", 1)
        if corrupted == generated:
            raise RuntimeError("compiled bridge negative control found no load branch")
        (rust_dir / "lifecycle_generated.rs").write_text(corrupted)
        if parse_vectors(probe(rustc, copy, temp / "mutated-decisions")) == expected:
            raise RuntimeError("compiled bridge failed to reject a corrupted load branch")
        evidence["completed_stages"].append("negative_controls")

    evidence["current_stage"] = "source_stability"
    observed_sources = snapshot(tracked)
    evidence["source_after_sha256"] = observed_sources
    require_same_source(before, observed_sources)
    evidence["completed_stages"].append("source_stability")
    return {
        "status": "passed",
        "proof_status": "passed",
        "proof_scope": "lifecycle decision refinement, effect cleanup ownership and atomic generation admission models",
        "theorems": names,
        "axiom_audit": axioms,
        "imported_module_trust_level": 0,
        "compiled_input_cases": len(expected),
        "negative_controls": {"false_theorem": "rejected", "mutated_rust_branch": "rejected"},
        "source_sha256": observed_sources,
        "generated_write_requested": args.write_generated,
        "current_stage": "complete",
        "integration_scope": "run the existing core lifecycle/effect/conformance tests separately",
        "unproved_boundaries": ["Rust adapter and state mutation correspondence", "callback and scheduler execution", "dependency resolution and committed binding implementation", "protocol, storage and external I/O"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lake", default="lake", help="Lake binary for the pinned Lean toolchain")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--rustfmt", default="rustfmt")
    parser.add_argument("--write-generated", action="store_true", help="regenerate the production decision module before checking")
    parser.add_argument("--evidence", type=Path, help="write source-bound JSON evidence")
    args = parser.parse_args()
    started = time.monotonic()
    evidence = {"status": "running", "proof_status": "pending", "completed_stages": []}
    exit_code = 0
    try:
        # Retire a previous passed receipt before starting a new attempt.
        write_evidence(args.evidence, evidence)
        evidence.update(verify(args, evidence))
    except (RuntimeError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        evidence.update(status="failed", proof_status="failed", error=str(error))
        print(f"semantic verification failed: {error}", file=sys.stderr)
        exit_code = 1
    evidence["elapsed_seconds"] = round(time.monotonic() - started, 3)
    try:
        write_evidence(args.evidence, evidence)
    except OSError as error:
        print(f"could not write semantic verification evidence: {error}", file=sys.stderr)
        return 1
    if exit_code:
        return exit_code
    print(f"Lean checked {len(evidence['theorems'])} theorems; compiled production decisions agree on all {evidence['compiled_input_cases']} input cases; both negative controls rejected.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
