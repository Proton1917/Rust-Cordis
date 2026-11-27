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
import tomllib

ROOT = Path(__file__).resolve().parents[1]
FORMAL = ROOT / "formal"
GENERATED = ROOT / "crates/cordis/src/lifecycle_generated.rs"
PROOFS = {
    "Lifecycle.lean": "Cordis",
    "Effects.lean": "Cordis.Effects",
    "Admission.lean": "Cordis.Admission",
    "Scopes.lean": "Cordis.Scopes",
    "DecisionProgram.lean": "Cordis.DecisionProgram",
    "KernelRefinement.lean": "Cordis.KernelRefinement",
    "Bindings.lean": "Cordis.Bindings",
    "Execution.lean": "Cordis.Execution",
    "NativeOwnership.lean": "Cordis.NativeOwnership",
    "Routing.lean": "Cordis.Routing",
    "Protocol.lean": "Cordis.Protocol",
    "Persistence.lean": "Cordis.Persistence",
    "SdkProtocol.lean": "Cordis.SdkProtocol",
    "Deployment.lean": "Cordis.Deployment",
    "Upstream.lean": "Cordis.Upstream",
    "KernelSemantics.lean": "Cordis.KernelSemantics",
    "BindingGraph.lean": "Cordis.BindingGraph",
    "UpstreamSimulation.lean": "Cordis.UpstreamSimulation",
    "Concurrency.lean": "Cordis.Concurrency",
    "AsyncSemantics.lean": "Cordis.AsyncSemantics",
    "WireSemantics.lean": "Cordis.WireSemantics",
    "RequestSemantics.lean": "Cordis.RequestSemantics",
    "CborSemantics.lean": "Cordis.CborSemantics",
    "StorageProgram.lean": "Cordis.StorageProgram",
    "StorageControlFlow.lean": "Cordis.StorageControlFlow",
    "DirectoryInitialization.lean": "Cordis.DirectoryInitialization",
    "Architecture.lean": "Cordis.Architecture",
    "Composition.lean": "Cordis.Composition",
}
PROGRAMS = {"Generate.lean", "Audit.lean", "KernelVectors.lean", "ProtocolProbe.lean",
            "UpstreamVectors.lean", "BindingGraphVectors.lean", "NativeVectors.lean",
            "StorageVectors.lean", "ArchitectureVectors.lean"}
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


def check_module_inventory() -> None:
    """Every local Lean input must belong to the audited, closed inventory."""
    expected = set(PROOFS) | PROGRAMS
    actual = {str(path.relative_to(FORMAL)) for path in FORMAL.rglob("*.lean")
              if ".lake" not in path.relative_to(FORMAL).parts}
    if actual != expected:
        raise RuntimeError(f"Lean module inventory mismatch: missing {sorted(expected - actual)}, "
                           f"unregistered {sorted(actual - expected)}")
    config = tomllib.loads((FORMAL / "lakefile.toml").read_text())
    targets = set(config.get("defaultTargets", []))
    libraries = {item["name"] for item in config.get("lean_lib", [])}
    required = {Path(path).stem for path in PROOFS} | {"Audit"}
    if not required <= targets or not required <= libraries:
        raise RuntimeError("every proof module and the auditor must be a default Lake build target")
    if json.loads((FORMAL / "lake-manifest.json").read_text()).get("packages") != []:
        raise RuntimeError("the audited project must use only the pinned Lean standard library")
    local = {Path(path).stem for path in expected}
    for path in sorted(expected):
        code = lean_code((FORMAL / path).read_text())
        for line in re.findall(r"^import\s+([^\n]+)", code, re.M):
            for module in line.split():
                if module not in local and module.split(".")[0] not in {"Init", "Std", "Lean"}:
                    raise RuntimeError(f"unregistered imported module in {path}: {module}")
        forbidden = re.search(r"\b(sorry|admit|axiom|native_decide|unsafe|implemented_by|skipKernelTC)\b", code)
        if forbidden:
            raise RuntimeError(f"proof escape in {path}: {forbidden.group(0)}")


def parse_declaration_audit(output: str, names: list[str]) -> list[dict]:
    records = [line.split("\t", 1)[1] for line in output.splitlines() if line.startswith("CORDIS_AUDIT\t")]
    if len(records) != 1:
        raise RuntimeError("kernel declaration audit did not emit exactly one census")
    declarations = json.loads(records[0])
    if not isinstance(declarations, list) or not declarations:
        raise RuntimeError("kernel declaration audit is empty")
    seen = set()
    modules = set()
    proved = set()
    for declaration in declarations:
        name, module = declaration["name"], declaration["module"]
        if name in seen:
            raise RuntimeError(f"duplicate kernel declaration in audit: {name}")
        seen.add(name)
        modules.add(module)
        actual = set(declaration["axioms"])
        # Lean's compiler creates unsafe stage definitions with erased proofs.
        # Their kernel flag, not a name pattern, distinguishes them from logic.
        # Compiler stages may contain erased-proof placeholders. Every theorem
        # and safe/private definition retains the standard-logic-only whitelist;
        # the kernel rejects any safe declaration that uses an unsafe constant.
        compiler_auxiliary = declaration["compiler_auxiliary"]
        if type(compiler_auxiliary) is not bool or type(declaration["theorem"]) is not bool:
            raise RuntimeError(f"invalid kernel declaration flags: {name}")
        if compiler_auxiliary and declaration["theorem"]:
            raise RuntimeError(f"invalid logical declaration classification: {name}")
        if not compiler_auxiliary and not actual <= ALLOWED_AXIOMS:
            raise RuntimeError(f"unapproved axiom dependency for {name}: {actual - ALLOWED_AXIOMS}")
        if declaration["theorem"]:
            proved.add(name)
    expected_modules = {Path(path).stem for path in PROOFS}
    if modules != expected_modules:
        raise RuntimeError(f"kernel audit module mismatch: {modules ^ expected_modules}")
    if not set(names) <= proved:
        raise RuntimeError(f"kernel audit omitted source theorems: {set(names) - proved}")
    return sorted(declarations, key=lambda declaration: declaration["name"])


def bridge_contracts() -> list[dict]:
    contracts = []
    for path in sorted((FORMAL / "bridges").glob("*.json")):
        document = json.loads(path.read_text())
        rows = document if isinstance(document, list) else document.get("bridges", document.get("domains", [document]))
        for row in rows:
            required = {"domain", "module", "production_paths", "entrypoints", "theorems", "assumptions", "evidence_commands"}
            if not required <= row.keys():
                raise RuntimeError(f"incomplete source bridge contract in {path.name}: {required - row.keys()}")
            if row["module"].removesuffix(".lean") not in {Path(name).stem for name in PROOFS}:
                raise RuntimeError(f"unknown proof module in {path.name}: {row['module']}")
            if not row["production_paths"] or not row["theorems"]:
                raise RuntimeError(f"empty source/theorem bridge in {path.name}")
            for filename in row["production_paths"]:
                source = ROOT / filename
                if source.is_symlink() or not source.is_file() or not source.resolve().is_relative_to(ROOT):
                    raise RuntimeError(f"source bridge file is unavailable: {filename}")
            contracts.append(row)
    if not contracts:
        raise RuntimeError("source bridge contracts are missing")
    return contracts


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
    formal_inputs = [path for path in FORMAL.rglob("*") if path.is_file()
                     and not any(part in {".lake", "__pycache__"} for part in path.relative_to(FORMAL).parts)
                     and path.suffix not in {".olean", ".ilean", ".pyc"}]
    contracts = bridge_contracts()
    # Whole-module probes import SDK support code and the compiled UP crate.
    # Bind their production dependency closure, including modules that a probe
    # does not extract directly, to the same before/after source snapshot.
    runtime_inputs = list(ROOT.glob("crates/*/src/**/*.rs")) + list(ROOT.glob("crates/*/Cargo.toml"))
    sdk_inputs = list((ROOT / "sdks/python/cordis_sdk").rglob("*.py"))
    sdk_inputs += list((ROOT / "sdks/typescript/cordis-sdk/src").rglob("*.ts"))
    return sorted(set(formal_inputs + runtime_inputs + sdk_inputs +
                      [ROOT / filename for row in contracts for filename in row["production_paths"]] + [
        GENERATED, ROOT / "Cargo.toml", ROOT / "Cargo.lock",
        ROOT / "rust-toolchain.toml", ROOT / "crates/cordis/src/lifecycle.rs",
        ROOT / "crates/cordis/src/fiber.rs", ROOT / "crates/cordis/src/effect.rs",
        ROOT / "crates/cordis/src/context.rs", ROOT / "crates/cordis/src/registry.rs",
        ROOT / "crates/cordis/tests/epoch_context.rs",
        ROOT / "crates/cordis/src/lib.rs", Path(__file__).resolve(),
        ROOT / "tools/protocol_storage_probe.py", ROOT / "tools/sdk_terminal_probe.py",
        ROOT / "tools/execution_semantics_probe.py", ROOT / "tools/storage_program_probe.py",
        ROOT / "tools/architecture_probe.py",
        ROOT / "sdks/python/requirements.txt", ROOT / "sdks/typescript/package-lock.json",
        ROOT / "tools/tests/test_verify_semantics.py",
        ROOT / ".github/workflows/conformance.yml",
    ]))


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


def validate_source_bridge(label: str, result: dict) -> None:
    count = result.get("compiled_input_cases")
    if result.get("status") != "passed" or type(count) is not int or count <= 0:
        raise RuntimeError(f"source bridge did not return complete evidence: {label}")
    if label == "protocol_storage" and not result.get("sdk", {}).get("cases"):
        raise RuntimeError("protocol bridge omitted the SDK execution traces")
    controls = result.get("negative_controls", {})
    if not isinstance(controls, dict) or not controls:
        raise RuntimeError(f"source bridge negative controls incomplete: {label}")
    for name, control in controls.items():
        status = control.get("status") if isinstance(control, dict) else control
        if not name or status != "rejected":
            raise RuntimeError(f"source bridge negative control incomplete: {label}/{name}")


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
    rust_version = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    if not evidence["rustc"].startswith(f"rustc {rust_version} "):
        raise RuntimeError(f"unexpected Rust version: {evidence['rustc']}; required {rust_version}")
    evidence["completed_stages"].append("toolchain")

    evidence["current_stage"] = "coverage_inventory"
    check_module_inventory()
    contracts = bridge_contracts()
    covered = {row["module"].removesuffix(".lean") for row in contracts}
    if covered != {Path(name).stem for name in PROOFS}:
        raise RuntimeError(f"source bridge coverage mismatch: {covered ^ {Path(name).stem for name in PROOFS}}")
    names = theorems()
    for row in contracts:
        if not set(row["theorems"]) <= set(names):
            raise RuntimeError(f"source bridge references unknown theorems: {set(row['theorems']) - set(names)}")
    evidence["completed_stages"].append("coverage_inventory")
    evidence["current_stage"] = "proof_build"
    tracked = source_paths()
    before = snapshot(tracked)
    evidence["source_before_sha256"] = dict(before)
    # A fresh proof build prevents stale .olean files from serving as evidence.
    run([lake, "clean"])
    build = run([lake, "build"])
    if re.search(r"\bsorry\b", build.stdout + build.stderr):
        raise RuntimeError("proof build contains an unresolved proof")
    evidence["completed_stages"].append("proof_build")
    with tempfile.TemporaryDirectory(prefix="cordis-semantics-") as raw_tmp:
        evidence["current_stage"] = "axiom_audit"
        temp = Path(raw_tmp)
        audit = temp / "Census.lean"
        imports = "import Audit\n" + "".join(f"import {Path(filename).stem}\n" for filename in PROOFS)
        modules = ", ".join(json.dumps(Path(filename).stem) for filename in PROOFS)
        audit.write_text(imports + f"run_cmd auditModules #[{modules}]\n")
        audit_output = run([lake, "env", "lean", "--trust=0", str(audit)]).stdout
        declarations = parse_declaration_audit(audit_output, names)
        declaration_modules = {declaration["name"]: declaration["module"] for declaration in declarations}
        for contract in contracts:
            module = contract["module"].removesuffix(".lean")
            wrong = [name for name in contract["theorems"] if declaration_modules[name] != module]
            if wrong:
                raise RuntimeError(f"source bridge theorem belongs to a different module: {wrong}")
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

        # Compiler-only declarations cannot provide proofs to the logical kernel.
        negative.write_text("unsafe def forged : False := unsafeCast ()\nexample : False := forged\n")
        rejected_unsafe = run([lake, "env", "lean", "--trust=0", str(negative)], success=False)
        if rejected_unsafe.returncode == 0 or "unsafe declaration" not in rejected_unsafe.stdout + rejected_unsafe.stderr:
            raise RuntimeError("kernel accepted a proof from a compiler-only declaration")

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

        bridges = {}
        for label, script, arguments in [
            ("kernel_refinement", FORMAL / "check_kernel_refinement.py", ["--lake", lake, "--rustc", rustc]),
            ("protocol_storage", ROOT / "tools/protocol_storage_probe.py", ["--lake", lake, "--rustc", rustc, "--node", args.node]),
            ("upstream", FORMAL / "check_upstream.py", ["--lake", lake, "--node", args.node]),
            ("binding_graph", FORMAL / "check_binding_graph.py", ["--lake", lake, "--rustc", rustc]),
            ("execution_semantics", ROOT / "tools/execution_semantics_probe.py", ["--lake", lake, "--rustc", rustc]),
            ("wire_semantics", FORMAL / "check_wire_semantics.py", ["--lake", lake, "--rustc", rustc, "--node", args.node]),
            ("cbor_semantics", FORMAL / "check_cbor_semantics.py", ["--lake", lake, "--rustc", rustc]),
            ("storage_program", ROOT / "tools/storage_program_probe.py", ["--lake", lake, "--rustc", rustc]),
            ("architecture", ROOT / "tools/architecture_probe.py", ["--lake", lake, "--rustc", rustc]),
        ]:
            evidence["current_stage"] = f"source_bridge_{label}"
            receipt = temp / f"{label}.json"
            run([sys.executable, str(script), *arguments, "--evidence", str(receipt)], cwd=ROOT)
            result = json.loads(receipt.read_text())
            validate_source_bridge(label, result)
            bridges[label] = result
            evidence["completed_stages"].append(f"source_bridge_{label}")

    evidence["current_stage"] = "source_stability"
    check_module_inventory()
    observed_sources = snapshot(source_paths())
    evidence["source_after_sha256"] = observed_sources
    require_same_source(before, observed_sources)
    evidence["completed_stages"].append("source_stability")
    observation_cases = {
        "lifecycle_decisions": len(expected),
        "kernel_boundaries": bridges["kernel_refinement"]["compiled_input_cases"],
        "protocol_budget": bridges["protocol_storage"]["compiled_input_cases"],
        "sdk_terminal_traces": bridges["protocol_storage"]["sdk"]["cases"],
        "upstream_lifecycle": bridges["upstream"]["compiled_input_cases"],
        "binding_graph": bridges["binding_graph"]["compiled_input_cases"],
        "execution_microsteps": bridges["execution_semantics"]["compiled_input_cases"],
        "wire_and_request": bridges["wire_semantics"]["compiled_input_cases"],
        "cbor_value_codec": bridges["cbor_semantics"]["compiled_input_cases"],
        "storage_io_and_recovery": bridges["storage_program"]["compiled_input_cases"],
        "supervisor_provenance": bridges["architecture"]["compiled_input_cases"],
    }
    return {
        "status": "passed",
        "proof_status": "passed",
        "proof_scope": [row["domain"] for row in contracts],
        "proof_modules": [Path(name).stem for name in PROOFS],
        "theorems": names,
        "audited_declaration_count": len(declarations),
        "audited_logical_declaration_count": sum(not item["compiler_auxiliary"] for item in declarations),
        "compiler_auxiliary_declaration_count": sum(item["compiler_auxiliary"] for item in declarations),
        "declaration_audit": declarations,
        "imported_module_trust_level": 0,
        "compiled_input_cases": len(expected),
        "source_observation_cases": observation_cases,
        "total_source_observations": sum(observation_cases.values()),
        "source_bridges": bridges,
        "bridge_contracts": contracts,
        "negative_controls": {"false_theorem": "rejected", "unsafe_proof_dependency": "rejected", "mutated_rust_branch": "rejected"},
        "source_sha256": observed_sources,
        "generated_write_requested": args.write_generated,
        "current_stage": "complete",
        "integration_scope": "production regression commands in bridge contracts run separately from this proof-and-source checker",
        "unproved_boundaries": [
            "complete Rust/JavaScript/Python language and compiler semantics; restricted source parsers, extraction adapters, and dependency implementations form the translation boundary",
            "std mutex/atomic/pointer contracts and valid foreign ABI ownership; the proved microstep model permits stale atomic observations but does not formalize the Rust memory model",
            "productive scheduler dispatch and callback return are explicit assumptions only for eventual cleanup, with arbitrary-schedule safety proved separately",
            "operating-system file-sync, directory-sync, atomic-rename, process/network and dynamic-loader contracts, under exclusive store ownership",
            "the recursive CBOR grammar is proved; ciborium/cbor2/JavaScript codec implementations, UTF-8 and floating-point conversion primitives have their declared dependency contracts",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lake", default="lake", help="Lake binary for the pinned Lean toolchain")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--rustfmt", default="rustfmt")
    parser.add_argument("--node", default="node", help="Node.js for the pinned upstream and SDK source bridges")
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
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
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
    observations = evidence["total_source_observations"]
    negative_count = len(evidence["negative_controls"]) + sum(
        len(bridge["negative_controls"]) for bridge in evidence["source_bridges"].values())
    print(f"Lean checked {len(evidence['theorems'])} theorems in {len(PROOFS)} modules; "
          f"audited {evidence['audited_logical_declaration_count']} logical declarations; "
          f"{observations} production-source observations agree; "
          f"all {negative_count} negative controls rejected.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
